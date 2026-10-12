//! First-run rig: a fresh engine (empty data dir) from start to the first
//! correct answer, in-process.
//!
//!   firstrun truth <data> <home> <out.json> <spec>...   build fully, record answers
//!   firstrun truth-names <data> <home> <out.json> <spec>...   record answers once the
//!       name index is in (greps then read the files: no content index is built)
//!   firstrun trial <data> <home> <truth.json> <spec>    time one spec's first correct answer
//!   firstrun full <data> <home>                         time a whole first run
//!   firstrun same <data> <home> <spec>...               first answers == built answers?
//!   firstrun seq <data> <home> <truth.json> <spec>...   ask each in turn until correct
//!   firstrun diff <data> <home> <truth.json> <spec>...  first answers vs the truth's, by path
//!   firstrun probe <data> <home> <probe.json> <root>    the bench's first-run probe ({q, target,
//!       pattern, files}: paths under root): name ranked first, then a grep holding every file
//!
//! spec: `name:<query>`, `grep:<limit>:<pattern> in:<folder>` (a literal,
//! no time budget). A trial asks again every 10 ms until the answer equals
//! the truth's. `<data>` must not exist. Folders macOS guards with a consent
//! prompt are skipped for the real home as well as `<home>` (the rig runs
//! without Full Disk Access; opening them could pop a prompt).

use fsearch::{Engine, GrepMode, Options, Query};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The daemon's allocator (src/main.rs), so footprints here are the daemon's:
/// big buffers straight from mmap and back with munmap.
struct Alloc;
const BIG: usize = 1 << 20;

unsafe impl std::alloc::GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        if l.size() < BIG || l.align() > 16384 {
            return unsafe { std::alloc::System.alloc(l) };
        }
        let p = unsafe { libc::mmap(std::ptr::null_mut(), l.size(), libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0) };
        if p == libc::MAP_FAILED { std::ptr::null_mut() } else { p as *mut u8 }
    }
    unsafe fn alloc_zeroed(&self, l: std::alloc::Layout) -> *mut u8 {
        if l.size() < BIG || l.align() > 16384 { unsafe { std::alloc::System.alloc_zeroed(l) } } else { unsafe { self.alloc(l) } }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        if l.size() < BIG || l.align() > 16384 {
            unsafe { std::alloc::System.dealloc(p, l) }
        } else {
            unsafe { libc::munmap(p as *mut libc::c_void, l.size()) };
        }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, new_size: usize) -> *mut u8 {
        if (l.size() < BIG && new_size < BIG) || l.align() > 16384 {
            return unsafe { std::alloc::System.realloc(p, l, new_size) };
        }
        let q = unsafe { self.alloc(std::alloc::Layout::from_size_align_unchecked(new_size, l.align())) };
        if !q.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(new_size));
                self.dealloc(p, l);
            }
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Alloc = Alloc;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (data, home) = (PathBuf::from(&a[1]), a[2].clone());
    assert!(!data.exists(), "{} exists: a first run needs an empty data dir", data.display());
    let t0 = Instant::now();
    let real = std::env::var("REAL_HOME").or_else(|_| std::env::var("HOME")).unwrap();
    let skip = fsearch::gated(&real).into_iter().chain(fsearch::gated(&home)).map(|p| PathBuf::from(String::from_utf8(p).unwrap())).collect();
    let e = Engine::start(Options { dir: data, home: home.clone(), skip: Some(skip) }).unwrap();
    let peak = sampler();
    match a[0].as_str() {
        "truth" | "truth-names" => {
            let (out, specs) = (&a[3], &a[4..]);
            let (ready, done) = if a[0] == "truth" {
                wait_built(&e, t0)
            } else {
                while !e.status().ready {
                    std::thread::sleep(Duration::from_millis(20));
                }
                (t0.elapsed().as_secs_f64(), 0.0)
            };
            let answers: serde_json::Map<String, Value> = specs.iter().map(|s| (s.clone(), ask(&e, s, &home).unwrap_or(Value::Null))).collect();
            std::fs::write(out, serde_json::to_vec_pretty(&answers).unwrap()).unwrap();
            println!("{}", json!({"ready_s": ready, "built_s": done}));
        }
        "trial" => {
            let truth: Value = serde_json::from_slice(&std::fs::read(&a[3]).unwrap()).unwrap();
            let spec = &a[4];
            let want = &truth[spec.as_str()];
            assert!(!want.is_null(), "no truth for {spec}");
            let (mut asks, mut first_ok) = (0, None);
            let mut first_any = None;
            while first_ok.is_none() && t0.elapsed() < Duration::from_secs(600) {
                asks += 1;
                let got = ask(&e, spec, &home);
                if got.is_ok() && first_any.is_none() {
                    first_any = Some(t0.elapsed().as_secs_f64());
                }
                if got.as_ref().is_ok_and(|g| g == want) {
                    first_ok = Some(t0.elapsed().as_secs_f64());
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            println!(
                "{}",
                json!({"spec": spec, "first_answer_s": first_any, "correct_s": first_ok, "asks": asks, "peak_mb": mb(peak.load(Ordering::Relaxed))})
            );
        }
        "seq" => {
            let truth: Value = serde_json::from_slice(&std::fs::read(&a[3]).unwrap()).unwrap();
            let mut last = 0.0;
            let mut out = Vec::new();
            for spec in &a[4..] {
                let want = &truth[spec.as_str()];
                assert!(!want.is_null(), "no truth for {spec}");
                while ask(&e, spec, &home).ok().as_ref() != Some(want) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let at = t0.elapsed().as_secs_f64();
                out.push(json!({"spec": spec, "at_s": at, "took_s": at - last}));
                last = at;
            }
            println!("{}", json!({"seq": out, "peak_mb": mb(peak.load(Ordering::Relaxed))}));
        }
        "same" => {
            // Each spec's first answer (asked as soon as one comes), then
            // the answer once everything is built: they must be equal.
            let first: Vec<(f64, Value)> = a[3..]
                .iter()
                .map(|s| {
                    loop {
                        if let Ok(v) = ask(&e, s, &home) {
                            break (t0.elapsed().as_secs_f64(), v);
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                })
                .collect();
            let (ready, done) = wait_built(&e, t0);
            let mut bad = 0;
            for (s, (at, v)) in a[3..].iter().zip(&first) {
                let built = ask(&e, s, &home).unwrap_or(Value::Null);
                let n = v.as_array().map_or(0, Vec::len);
                if &built != v {
                    bad += 1;
                    let (x, y) = (v.as_array().cloned().unwrap_or_default(), built.as_array().cloned().unwrap_or_default());
                    let k = x.iter().zip(&y).position(|(a, b)| a != b).unwrap_or(x.len().min(y.len()));
                    let sorted = |mut v: Vec<Value>| {
                        v.sort_by_key(|x| x.to_string());
                        v
                    };
                    let same_set = sorted(x.clone()) == sorted(y.clone());
                    println!(
                        "DIFF {s} (first at {at:.3}s, {n} results; built {}; same set {same_set}; first diff at {k}: {} vs {})",
                        y.len(),
                        x.get(k).map_or(String::new(), |v| v.to_string().chars().take(200).collect()),
                        y.get(k).map_or(String::new(), |v| v.to_string().chars().take(200).collect())
                    );
                } else {
                    println!("same {s} (first at {at:.3}s, {n} results)");
                }
            }
            println!("{}", json!({"specs": first.len(), "differ": bad, "ready_s": ready, "built_s": done}));
        }
        "probe" => {
            // As the bench probes a first run (bench/fsrun): the name query, in
            // <root>, until its target ranks first; then every match of the
            // pattern until the answer holds each listed file. Times from start.
            let p: Value = serde_json::from_slice(&std::fs::read(&a[3]).unwrap()).unwrap();
            let root = &a[4];
            let s = |k: &str| p[k].as_str().unwrap().to_string();
            let target = format!("{root}/{}", s("target"));
            let want: std::collections::BTreeSet<String> =
                p["files"].as_array().unwrap().iter().map(|f| format!("{root}/{}", f.as_str().unwrap())).collect();
            let deadline = Duration::from_secs(600);
            let mut name_s = None;
            while name_s.is_none() && t0.elapsed() < deadline {
                let mut q = Query::parse(&format!("in:{root} {}", s("q")), &home).unwrap();
                q.limit = 50;
                if e.search(&q).is_ok_and(|f| f.first().is_some_and(|f| f.path.to_string_lossy() == target)) {
                    name_s = Some(t0.elapsed().as_secs_f64());
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let mut grep_s = None;
            while grep_s.is_none() && t0.elapsed() < deadline {
                let mut q = Query::parse(&format!("in:{root}"), &home).unwrap();
                q.limit = usize::MAX >> 1;
                let mut g = fsearch::Grep::new(&s("pattern"), GrepMode::Literal).unwrap();
                (g.max_per_file, g.budget) = (1, None);
                let r = e.grep(&q, &g);
                let got: std::collections::BTreeSet<String> =
                    r.as_ref().map(|(r, _)| r.files.iter().map(|f| String::from_utf8_lossy(&f.path).into_owned()).collect()).unwrap_or_default();
                if std::env::var_os("PROBE_TRACE").is_some() {
                    let missing: Vec<&String> = want.difference(&got).collect();
                    match &r {
                        Ok((r, ix)) => eprintln!(
                            "probe {:.2}s: {} files, complete {}, read {}, indexed {ix}, missing {}: {:?}",
                            t0.elapsed().as_secs_f64(),
                            r.files.len(),
                            r.complete,
                            r.read,
                            missing.len(),
                            missing.iter().take(3).collect::<Vec<_>>()
                        ),
                        Err(x) => eprintln!("probe {:.2}s: error {x}", t0.elapsed().as_secs_f64()),
                    }
                }
                if want.is_subset(&got) {
                    grep_s = Some(t0.elapsed().as_secs_f64());
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            println!("{}", json!({"name_s": name_s, "grep_s": grep_s, "peak_mb": mb(peak.load(Ordering::Relaxed))}));
        }
        "diff" => {
            // Each spec's first answer vs the truth's, by path: missing, extra, lines differing.
            let truth: Value = serde_json::from_slice(&std::fs::read(&a[3]).unwrap()).unwrap();
            for spec in &a[4..] {
                let got = loop {
                    if let Ok(v) = ask(&e, spec, &home) {
                        break v;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                let at = t0.elapsed().as_secs_f64();
                let by = |v: &Value| -> std::collections::BTreeMap<String, Value> {
                    v.as_array().into_iter().flatten().map(|x| (x[0].as_str().unwrap_or("").to_string(), x[1].clone())).collect()
                };
                let (w, g) = (by(&truth[spec.as_str()]), by(&got));
                let missing: Vec<&String> = w.keys().filter(|k| !g.contains_key(*k)).collect();
                let extra: Vec<&String> = g.keys().filter(|k| !w.contains_key(*k)).collect();
                let lines = w.iter().filter(|(k, v)| g.get(*k).is_some_and(|x| x != *v)).count();
                let order = got == truth[spec.as_str()];
                println!(
                    "{spec}: at {at:.2}s, {} files vs truth {}: missing {}, extra {}, lines differ {lines}, identical {order}",
                    g.len(),
                    w.len(),
                    missing.len(),
                    extra.len()
                );
                for p in missing.iter().take(8) {
                    println!("  missing {p}");
                }
                for p in extra.iter().take(8) {
                    println!("  extra {p}");
                }
            }
        }
        "full" => {
            let (ready, done) = wait_built(&e, t0);
            let content_peak = mb(CONTENT_PEAK.load(Ordering::Relaxed));
            let (user, sys) = cpu();
            let s = e.status();
            #[rustfmt::skip]
            println!("{}", json!({"ready_s": ready, "built_s": done, "peak_mb": mb(lifetime_peak()), "sampled_peak_mb": mb(peak.load(Ordering::Relaxed)), "content_peak_mb": content_peak, "now_mb": mb(footprint().0), "user_s": user, "sys_s": sys, "entries": s.entries, "content_docs": s.content_docs}));
        }
        _ => panic!("unknown command"),
    }
    // Exit without dropping the engine (it has no stop).
    std::process::exit(0);
}

/// The sampled peak footprint since the name index was ready.
static CONTENT_PEAK: AtomicU64 = AtomicU64::new(0);
static READY: AtomicBool = AtomicBool::new(false);

/// Seconds to the name index being ready, then to the content index done.
fn wait_built(e: &Engine, t0: Instant) -> (f64, f64) {
    let mut ready = None;
    loop {
        let s = e.status();
        if s.ready && ready.is_none() {
            ready = Some(t0.elapsed().as_secs_f64());
            READY.store(true, Ordering::Relaxed);
        }
        if s.ready && s.content_docs > 0 && s.content_pending == 0 {
            return (ready.unwrap(), t0.elapsed().as_secs_f64());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One spec's answer, without timings.
fn ask(e: &Engine, spec: &str, home: &str) -> Result<Value, String> {
    if let Some(q) = spec.strip_prefix("name:") {
        let found = e.search(&Query::parse(q, home)?)?;
        return Ok(found.iter().map(|f| json!([f.path.to_string_lossy(), f.score])).collect());
    }
    let rest = spec.strip_prefix("grep:").ok_or("bad spec")?;
    let (limit, rest) = rest.split_once(':').ok_or("bad grep spec")?;
    let (pattern, scope) = rest.rsplit_once(" in:").ok_or("grep spec needs in:")?;
    let mut q = Query::parse(&format!("in:{scope}"), home)?;
    q.limit = limit.parse().map_err(|_| "bad limit")?;
    let mut g = fsearch::Grep::new(pattern, GrepMode::Literal)?;
    g.budget = None;
    let (r, _) = e.grep(&q, &g)?;
    if !r.complete {
        return Err("incomplete".into());
    }
    Ok(r.files.iter().map(|f| json!([String::from_utf8_lossy(&f.path), f.lines])).collect())
}

fn mb(b: u64) -> f64 {
    (b as f64 / 1048576.0 * 10.0).round() / 10.0
}

/// (phys_footprint, lifetime max) of this process.
fn footprint() -> (u64, u64) {
    let mut ri: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, (&raw mut ri).cast()) };
    (ri.ri_phys_footprint, ri.ri_lifetime_max_phys_footprint)
}

fn lifetime_peak() -> u64 {
    footprint().1
}

/// Peak footprint, sampled every 20 ms (the lifetime max also counts
/// transient spikes). With FIRSTRUN_TRACE set, also prints the footprint
/// every 100 ms to stderr (`trace <s> <MB>`).
fn sampler() -> &'static AtomicU64 {
    static PEAK: AtomicU64 = AtomicU64::new(0);
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::Relaxed) {
        let trace = std::env::var_os("FIRSTRUN_TRACE").is_some();
        let t0 = Instant::now();
        std::thread::spawn(move || {
            for k in 0u64..=u64::MAX {
                let f = footprint().0;
                PEAK.fetch_max(f, Ordering::Relaxed);
                if READY.load(Ordering::Relaxed) {
                    CONTENT_PEAK.fetch_max(f, Ordering::Relaxed);
                }
                if trace && (k % 5 == 0 || f >= PEAK.load(Ordering::Relaxed)) {
                    eprintln!("trace {:.1} {}", t0.elapsed().as_secs_f64(), mb(f));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }
    &PEAK
}

fn cpu() -> (f64, f64) {
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
    let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    (s(r.ru_utime), s(r.ru_stime))
}
