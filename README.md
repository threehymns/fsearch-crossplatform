# FSearch Crossplatform

Whole-disk file search for macOS and Linux: names in about 0.1 ms, typo-tolerant,
plus indexed search inside files. A CLI with a small daemon, or a Rust crate.
A cross-platform fork of [FSearch](https://github.com/noahdunnagan/fsearch).

```bash
# Install via Cargo
cargo install fsearch-crossplatform
# or instant precompiled binary via cargo-binstall:
cargo binstall fsearch-crossplatform

# Arch Linux
cd packaging && makepkg -si

# Debian / Ubuntu (.deb from GitHub Releases)
sudo dpkg -i fsearch-crossplatform_*.deb

# Or build from source
cargo build --release && ./target/release/fsearch install   # -> ~/.local/bin/fsearch

# Use
fsearch fsearch main              # find files by name
fsearch 'ext:rs grep:apply_dir'   # search inside files
```

## Speed

M4 Max, 8.3M files on disk, vs the previous version (76d612f). Same
results. Medians.

| | before | now | |
|---|---:|---:|---:|
| find a file by name | 1.0 ms | 0.13 ms | 7.7× |
| typing a whole filename, all keystrokes | 23 ms | 3.0 ms | 7.5× |
| search inside files | 14 ms | 2.3 ms | 6.2× |
| slowest 10% inside files (Chromium) | 38 ms | 2.3 ms | 16× |
| CLI, launch to answer | 5.5 ms | 3.3 ms | 1.7× |
| first run, fully searchable | 101 s | 49 s | 2.1× |
| first run, peak memory | 1.2 GB | 0.9 GB | |
| index on disk (Chromium) | 0.32 GB | 0.76 GB | 2.4× bigger |

The index grew because each file carries a filter that lets search skip
files without the text.

On Linux (i7-8565U, Chromium tree at 509k files, method:
[`demo/bench_linux.py`](demo/bench_linux.py): 0.18 ms by name, 1.3 ms
inside files, typo still first 99.5%, ready after restart ~15 ms. The
on-disk index is byte-identical with macOS (777 MB here against
0.76 GB there).

## Queries

```
fsearch 'readme in:~/Developer'          # inside a folder
fsearch 'type:image size:>5mb mtime:<7d'
fsearch 'ext:rs regex:fn\s+\w+_dir'      # regex inside files
fsearch 'sym:apply_dir'                  # where it's defined
```

Words are fuzzy, and 5+ letter words forgive one typo (`mian.rs` finds
`main.rs`). Also `'exact`, `^prefix`, `suffix$` and `!exclude`. Filters:
`ext:` `type:` `kind:` `in:` `size:` `mtime:` `re:` `path:` `grep:` `regex:`
`sym:` `limit:`. Content search is smart-case.

## Full Disk Access

From a terminal with Full Disk Access it indexes everything. As a login
item (`fsearch install --login`), grant `~/.local/bin/fsearch` access in
System Settings > Privacy & Security, again after each rebuild. Without
access it skips protected folders instead of prompting.

## Goals of this Fork

- No change in behavior or logic on macOS compared to upstream.
- Support Linux and other platforms with parity to upstream where possible.
- Drop-in support as a dep for apps already using upstream with the same
binary-name (`fsearch`) as upstream.
- Staying up to date with the direction taken upstream.

## Non-Goals

- Adding GUI, TUI, or other features not present upstream beyond what's
needed for cross-platform parity.
- Bugfixes in platform-shared code before upstream gets them.

## Linux

Indexes `$HOME` with `getdents64` and watches it with inotify. State lives in
`$XDG_DATA_HOME/fsearch`, else `~/.local/share/fsearch`. `fsearch install
--login` installs a systemd user unit. There is no event replay. Restarts
recover with a `synced_at` mtime relist.

### Limits

Each watched directory costs one inotify watch, so very large trees
can exceed `max_user_watches` (a developer home here needs ~465k against
Arch's 524k default; lower-default systems will hit the wall, so to fix you
can raise that and restart the daemon). Bursts of files in brand-new directories
can outrun watch establishment and surface on the next change or restart.
Cross-device mounts under home (like a flash drive mounted to the path `~/external`)
are skipped by the scan. Plan disk and memory with the index: a 556k-entry
tree costs about 0.8 GB of index files and on the order of 0.5 GB
steady-state daemon RSS after heavy searching.

### Building

Release binaries stay portable across `x86_64` machines. For a faster
binary on the machine you build on, see `pgo.sh` (profile-guided
optimization, portable) and the `target-cpu=native` recipe in its header
(self-builds only — never ship native-tuned binaries to other machines).

## API

JSON lines over `fsearch.sock` in the data dir, or `fsearch stdio`:

```json
{"q": "fsearch main", "limit": 20}
{"op": "grep", "pattern": "apply_dir", "in": "~/Developer"}
```

Or link the crate in `Cargo.toml`:

```toml
[dependencies]
fsearch = { version = "0.1", package = "fsearch-crossplatform" }
```

And use it in your code:

```rust
let engine = fsearch::Engine::start(fsearch::Options { dir: fsearch::default_dir(&home), home: home.clone(), skip: None })?;
let hits = engine.search(&fsearch::Query::parse("fsearch main", &home)?)?;
```

An app and the CLI share one index: the first process owns it, the rest
follow.

## How it works

- Crawls the disk once with `getattrlistbulk`, then follows FSEvents:
  changes show up in about 0.1 s, and a restart replays only what changed.
- Names live in one mmap'd file, folder by folder, so `in:` is a range. A
  bitmap of each name's letters rules out most names before scoring.
- Content search is a trigram index plus a per-file filter, so a search
  only reads files likely to match. Identical files are indexed once.
  Matches are read fresh from disk.
