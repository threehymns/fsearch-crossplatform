//! Name-search quality rig: how high fsearch ranks the file a query means.
//!
//!   quality snap <snap dir> <home>                    crawl the disk into <snap>/index.bin
//!   quality run <snap dir> <root> <queries.json> <out.json>
//!   quality ask <snap dir> <query>                    print the hits' paths
//!
//! `snap` skips the folders macOS guards with a consent prompt (for the real
//! home too: no Full Disk Access here). `run` asks each query scoped to
//! `<root>` and records where the wanted paths (relative to it) rank; the
//! queries and fff's answers come from examples/quality.py.

use fsearch::index::Index;
use fsearch::live::Live;
use fsearch::query::{Query, Searcher};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("snap") => snap(Path::new(&a[1]), &a[2]),
        Some("run") => run(Path::new(&a[1]), &a[2], &a[3], &a[4]),
        Some("ask") => ask(Path::new(&a[1]), &a[2]),
        _ => eprintln!("usage: quality snap <snap> <home> | quality run <snap> <root> <queries.json> <out.json> | quality ask <snap> <query>"),
    }
}

fn snap(dir: &Path, home: &str) {
    let real = std::env::var("REAL_HOME").or_else(|_| std::env::var("HOME")).unwrap();
    let _ = fsearch::walk::SKIP.set(fsearch::gated(&real).into_iter().chain(fsearch::gated(home)).collect());
    let t = Instant::now();
    let ls = fsearch::walk::scan(b"/", 8);
    let idx = Index::build(ls, 0, fsearch::query::now_secs(), home.as_bytes());
    std::fs::create_dir_all(dir.join("content")).unwrap();
    idx.save(&dir.join("index.bin")).unwrap();
    eprintln!("{} entries, {} names, {:.1} s", idx.n, idx.u, t.elapsed().as_secs_f64());
}

fn ask(snap: &Path, q: &str) {
    let live = Live::new(Index::load(&snap.join("index.bin")).expect("index"));
    let q = Query::parse(q, &std::env::var("HOME").unwrap()).unwrap();
    let mut p = Vec::new();
    for h in (Searcher { live: &live }).search(&q) {
        live.base.path(h.idx as usize, &mut p);
        println!("{}\t{}", String::from_utf8_lossy(&p), live.base.kind(h.idx as usize) & 3);
    }
}

fn run(snap: &Path, root: &str, queries: &str, out: &str) {
    let home = std::env::var("HOME").unwrap();
    let live = Live::new(Index::load(&snap.join("index.bin")).expect("index"));
    let qs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(queries).unwrap()).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().build().unwrap();
    let mut p = Vec::new();
    let res: Vec<Value> = qs
        .iter()
        .map(|q| {
            let parsed = Query::parse(&format!("in:{root} {}", q["q"].as_str().unwrap()), &home).unwrap();
            let t = Instant::now();
            let hits = pool.install(|| Searcher { live: &live }.search(&parsed));
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let paths: Vec<String> = hits
                .iter()
                .map(|h| {
                    live.base.path(h.idx as usize, &mut p);
                    String::from_utf8_lossy(&p[root.len() + 1..]).into_owned()
                })
                .collect();
            let want: Vec<&str> = q["want"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
            let rank = paths.iter().position(|p| want.contains(&p.as_str())).map_or(0, |r| r + 1);
            json!({"rank": rank, "ms": ms, "top": &paths[..paths.len().min(3)]})
        })
        .collect();
    std::fs::write(out, serde_json::to_string(&res).unwrap()).unwrap();
}
