//! Content freshness rig: how soon text written to a file is greppable, in
//! a real engine (FSEvents, the content worker).
//!
//!   fresh <data dir> <home> <trials>
//!
//! The engine's "whole disk" is just the path down to `<home>` (every other
//! folder on the way is skipped), so `<home>` should be a small scratch
//! folder. Each trial writes a new token, alternately into a new file and
//! appended to a file the index already holds, then greps until it is found.

use fsearch::{Engine, Grep, GrepMode, Options, Query};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (data, home, trials) = (PathBuf::from(&a[0]), a[1].clone(), a[2].parse::<usize>().unwrap());
    assert!(!data.exists(), "{} exists: use a fresh data dir", data.display());
    let mut skip = Vec::new();
    let mut at = PathBuf::from("/");
    for comp in Path::new(&home).components().skip(1) {
        let next = at.join(comp);
        skip.extend(std::fs::read_dir(&at).unwrap().flatten().map(|e| e.path()).filter(|p| *p != next));
        at = next;
    }
    let e = Engine::start(Options { dir: data, home: home.clone(), skip: Some(skip) }).unwrap();
    let t0 = Instant::now();
    loop {
        let s = e.status();
        if s.ready && s.content_docs > 0 && s.content_pending == 0 {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(120), "no content index after 2 min");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Let the first sync's own FSEvents settle.
    std::thread::sleep(Duration::from_secs(3));
    let old = Path::new(&home).join("held.txt");
    let mut ms = Vec::new();
    for trial in 0..trials {
        let token = format!("zqfresh{trial}x{}", t0.elapsed().as_nanos());
        let path = if trial % 2 == 0 { Path::new(&home).join(format!("new-{trial}.txt")) } else { old.clone() };
        let t = Instant::now();
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(&format!("a line with {token} in it\n"));
        std::fs::write(&path, text).unwrap();
        let (g, q) = (Grep::new(&token, GrepMode::Literal).unwrap(), Query::parse("", &home).unwrap());
        let found = loop {
            if e.grep(&q, &g).is_ok_and(|(r, _)| !r.files.is_empty()) {
                break Some(t.elapsed());
            }
            if t.elapsed() > Duration::from_secs(10) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        ms.push(found.map_or(f64::INFINITY, |d| d.as_secs_f64() * 1e3));
        // Apart enough that each trial's write is its own change.
        std::thread::sleep(Duration::from_millis(3000));
    }
    let mut sorted = ms.clone();
    sorted.sort_by(f64::total_cmp);
    println!("{}", serde_json::json!({"trials": ms, "p50_ms": sorted[sorted.len() / 2], "max_ms": sorted[sorted.len() - 1]}));
    // Exit without dropping the engine (it has no stop).
    std::process::exit(0);
}
