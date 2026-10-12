//! Churn rig: what the content index keeps for deleted and superseded files
//! over simulated weeks, and what compaction costs. The content side of the
//! daemon runs in-process and deterministically: each round's file changes
//! reach the name index as FSEvents would hand them over (each touched
//! folder relisted), then one content sync like the content worker's
//! (diff, segment builds, merges until the plan says stop).
//!
//!   churn <work dir> <index.bin> <corpus> <days> [rounds a day]
//!
//! <work dir> must not exist; it gets home/ (an APFS clone of <corpus>) and
//! content/. A day: EDITS files edited (a line appended), CREATES new files
//! and as many deleted, LOGS notes files appended to every round (as a
//! transcript or log grows), and every 7th day a checkout editing CHECKOUT
//! files at once. One JSON line per day; at the end, every live doc merged
//! into one segment: what the index would be with nothing dead.
#![allow(clippy::type_complexity)]

use fsearch::Grep;
use fsearch::content::{self, Content};
use fsearch::index::Index;
use fsearch::live::Live;
use fsearch::query::Query;
use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

const EDITS: usize = 300;
const CREATES: usize = 40;
const LOGS: usize = 6;
const CHECKOUT: usize = 1500;

#[derive(Default)]
struct Stats {
    built: u64,
    merged: u64,
    build_s: f64,
    merge_s: f64,
    merges: usize,
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (work, days) = (PathBuf::from(&a[0]), a[3].parse::<usize>().unwrap());
    let rounds: usize = a.get(4).map_or(12, |r| r.parse().unwrap());
    assert!(!work.exists(), "{} exists", work.display());
    std::fs::create_dir_all(&work).unwrap();
    let home = work.join("home");
    assert!(std::process::Command::new("cp").arg("-cR").arg(&a[2]).arg(&home).status().unwrap().success());
    std::fs::create_dir_all(home.join("notes")).unwrap();
    let hb = home.as_os_str().as_bytes().to_vec();
    let mut live = Live::new(Index::load(a[1].as_ref()).expect("index"));
    // The new home, as an event on its parent would add it.
    let f = live.fetch(&hb, true);
    live.apply(f);
    live.trees.clear();
    let mut c = Content::open(work.join("content"));
    let mut st = Stats::default();
    sync(&mut c, &live, &hb, &[], std::slice::from_ref(&hb), &mut st);
    report(0, &c, &st);
    let mut files = Vec::new();
    text_files(&home, &mut files);
    files.sort();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut made = 0;
    for day in 1..=days {
        for round in 0..rounds {
            // The folders this round changes files in (as FSEvents reports them).
            let mut touched = BTreeSet::new();
            let share = |n: usize| n / rounds + (round < n % rounds) as usize;
            let checkout = if day % 7 == 0 && round == 0 { CHECKOUT } else { 0 };
            for k in 0..share(EDITS) + checkout {
                let p = &files[rng.below(files.len())];
                append(p, &format!("\n/* churn {day}.{round}.{k} */\n"));
                touched.insert(folder(p));
            }
            for k in 0..share(CREATES) {
                let gone = files.swap_remove(rng.below(files.len()));
                std::fs::remove_file(&gone).unwrap();
                touched.insert(folder(&gone));
                let from = &files[rng.below(files.len())];
                let ext = from.extension().map_or(String::new(), |e| format!(".{}", e.to_string_lossy()));
                let to = from.with_file_name(format!("churn_{made}{ext}"));
                made += 1;
                std::fs::copy(from, &to).unwrap();
                append(&to, &format!("\n/* new {day}.{round}.{k} */\n"));
                touched.insert(folder(&to));
                files.push(to);
            }
            for k in 0..LOGS {
                let line: String = (0..24).map(|j| format!("{day}.{round}.{j} step {} done; ", rng.below(1 << 20))).collect();
                let p = home.join(format!("notes/log{k}.md"));
                append(&p, &(line + "\n"));
                touched.insert(folder(&p));
            }
            let dirs: Vec<Vec<u8>> = touched.into_iter().collect();
            for d in &dirs {
                let f = live.fetch(d, false);
                live.apply(f);
            }
            let trees = std::mem::take(&mut live.trees);
            sync(&mut c, &live, &hb, &dirs, &trees, &mut st);
        }
        report(day, &c, &st);
    }
    // A fresh index of the same files must answer the same: rare words from
    // random files, compared as sets (tie order follows segment layout).
    let mut fresh = Content::open(work.join("fresh"));
    sync(&mut fresh, &live, &hb, &[], std::slice::from_ref(&hb), &mut Stats::default());
    let word = regex::Regex::new(r"\b[a-z][a-z0-9_]{9,30}\b").unwrap();
    let (mut asked, mut differ) = (0, 0);
    while asked < 40 {
        let Ok(body) = std::fs::read_to_string(&files[rng.below(files.len())]) else { continue };
        let found: Vec<&str> = word.find_iter(&body).map(|m| m.as_str()).collect();
        if found.is_empty() {
            continue;
        }
        let q = format!("grep:{}", found[rng.below(found.len())]);
        asked += 1;
        let (a, b) = (grep(&c, &q, &home), grep(&fresh, &q, &home));
        if a != b {
            differ += 1;
            let only = |x: &[(Vec<u8>, Vec<(usize, String)>)], y: &[(Vec<u8>, Vec<(usize, String)>)]| {
                x.iter().filter(|f| !y.contains(f)).map(|f| String::from_utf8_lossy(&f.0).into_owned()).take(3).collect::<Vec<_>>()
            };
            eprintln!("DIFF {q}: churned {} files, fresh {}; only churned {:?}; only fresh {:?}", a.len(), b.len(), only(&a, &b), only(&b, &a));
        }
    }
    // Every live doc in one segment: the bytes with nothing dead.
    let (id, t) = (c.alloc_id(), Instant::now());
    let all: Vec<u64> = c.segs.iter().map(|s| s.id).collect();
    let merged = content::merge(&c.dir, id, &c.segments(&all)).map_or(0, |_| seg_bytes(&c.dir, id));
    #[rustfmt::skip]
    println!("{}", json!({"compacted_bytes": merged, "now_bytes": c.bytes(), "fresh_bytes": fresh.bytes(), "merge_all_s": t.elapsed().as_secs_f64(),
        "greps": asked, "greps_differ": differ}));
}

/// A grep's matches (path, lines) as a sorted list.
fn grep(c: &Content, q: &str, home: &Path) -> Vec<(Vec<u8>, Vec<(usize, String)>)> {
    let mut q = Query::parse(q, home.to_str().unwrap()).unwrap();
    let mut g = Grep::new(&q.grep.take().unwrap(), q.grep_mode).unwrap();
    // Every match: no time budget, no page limit.
    (g.budget, q.limit) = (None, 1 << 20);
    let mut out: Vec<_> = c.search(&g, &q).files.into_iter().map(|f| (f.path, f.lines)).collect();
    out.sort();
    out
}

/// One content sync, as the content worker runs it.
fn sync(c: &mut Content, live: &Live, home: &[u8], dirs: &[Vec<u8>], trees: &[Vec<u8>], st: &mut Stats) {
    let todo = c.diff(content::wants(live, home, dirs, trees));
    let t = Instant::now();
    for range in todo.batches() {
        let id = c.alloc_id();
        if let Some(seg) = content::build_segment(&c.dir, id, &todo, range) {
            st.built += seg_bytes(&c.dir, id);
            c.push(seg);
        }
    }
    st.build_s += t.elapsed().as_secs_f64();
    while let Some(ids) = c.merge_plan() {
        let (id, t) = (c.alloc_id(), Instant::now());
        let Some(seg) = content::merge(&c.dir, id, &c.segments(&ids)) else { break };
        st.merged += seg_bytes(&c.dir, id);
        st.merge_s += t.elapsed().as_secs_f64();
        st.merges += 1;
        c.replace(&ids, seg);
    }
}

fn append(p: &Path, s: &str) {
    std::fs::OpenOptions::new().append(true).create(true).open(p).unwrap().write_all(s.as_bytes()).unwrap();
}

fn folder(p: &Path) -> Vec<u8> {
    p.parent().unwrap().as_os_str().as_bytes().to_vec()
}

fn seg_bytes(dir: &Path, id: u64) -> u64 {
    std::fs::metadata(dir.join(format!("seg-{id:06}.fsc"))).map_or(0, |m| m.len())
}

fn report(day: usize, c: &Content, st: &Stats) {
    let (docs, live): (usize, usize) = (c.segs.iter().map(|s| s.ndocs).sum(), c.docs());
    let dead: usize = c.segs.iter().map(|s| s.dead_bytes()).sum();
    #[rustfmt::skip]
    println!("{}", json!({"day": day, "bytes": c.bytes(), "segs": c.segs.len(), "docs": docs, "live": live, "dead_est": dead,
        "built": st.built, "merged": st.merged, "merges": st.merges, "build_s": st.build_s, "merge_s": st.merge_s}));
}

/// Files the content index takes, by extension.
fn text_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let (p, t) = (e.path(), e.file_type().unwrap());
        if t.is_dir() && !t.is_symlink() {
            text_files(&p, out);
        } else if t.is_file() && p.extension().is_some_and(|x| ["c", "h", "rst", "txt", "py", "sh", "yaml"].iter().any(|y| x == *y)) {
            out.push(p);
        }
    }
}
