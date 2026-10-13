# FSearch Crossplatform

Whole-disk file search for macOS and Linux: names in about 0.1 ms, typo-tolerant,
plus indexed search inside files. A CLI with a small daemon, a Rust crate,
or an MCP server for coding agents.
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

Or skip the Rust toolchain with a release build, Apple Silicon or x86_64 Linux:

```
curl -L https://github.com/threehymns/fsearch-crossplatform/releases/latest/download/fsearch-aarch64-apple-darwin.tar.gz | tar xz && ./fsearch install
curl -L https://github.com/threehymns/fsearch-crossplatform/releases/latest/download/fsearch-x86_64-unknown-linux-gnu.tar.gz | tar xz && ./fsearch install
```

## Speed

M4 Mac mini, a home holding the Linux or the Chromium source tree, vs the
previous version (b88f4e9). Medians; Linux / Chromium.

| | before | now |
|---|---:|---:|
| right file first, name search | 83% / 86% | 93% / 97% |
| right file first, two typos | 11% / 15% | 74% / 93% |
| find a file by name | 0.045 / 0.10 ms | 0.054 / 0.12 ms |
| an edited file greppable | 2.1 s | 1.3 ms |
| a rename or delete visible | 45 / 47 ms | 18 / 15 ms |
| first run, a name in a folder | 7.8 / 7.8 s | 0.18 / 1.2 s |
| first run, every match in the tree | 10.9 / 12.2 s | 2.4 / 10.8 s |
| files grep can search (rg's own set) | 96% / 92% | 99.9% / 99.8% |
| grep, first page | 0.29 / 0.48 ms | 0.33 / 0.65 ms |
| index on disk | 503 / 893 MB | 309 / 600 MB |
| peak memory while indexing | 1.13 / 1.37 GB | 0.77 / 0.94 GB |
| daemon memory | 19 / 100 MB | 21 / 35 MB |

The index is a third smaller while holding a third more text: one 3-bit
filter per file instead of two, compressed posting lists and paths, and
bit-packed name arrays read in place. Grep's first page reads a few more
files for it, and the deeper typo search costs a little on names.

On Linux (i7-8565U, Chromium tree at 509k files, method:
[`demo/bench_linux.py`](demo/bench_linux.py), hot cache): ~0.2 ms by name,
~2–5 ms inside files, typo still first 99.5%, ready after restart ~20 ms. On
macOS the on-disk index is byte-identical with upstream (0.76 GB there); on Linux it runs a
touch larger (777 MB here), since each entry carries a fifth kind bit for graft prefixes above
home — the two platforms' saves are not interchangeable.

## Queries

```
fsearch 'readme in:~/Developer'          # inside a folder
fsearch 'type:image size:>5mb mtime:<7d'
fsearch 'ext:rs regex:fn\s+\w+_dir'      # regex inside files
fsearch 'sym:apply_dir'                  # where it's defined
fsearch 'src/**/*.{c,h} !test/ init'     # globs, in or out
fsearch 'git:modified in:~/Developer/app'
```

Words are fuzzy and forgive a typo at any letter, two in words of 8+
(`mian.rs` finds `main.rs`); several words can spell one name. Accented
names match however they were stored. Also `'exact`, `^prefix`, `suffix$`
and `!exclude`. Filters: `ext:` `type:` `kind:` `in:` `size:` `mtime:`
`re:` `path:` `grep:` `regex:` `sym:` `limit:` and `git:` (`modified`
`staged` `untracked` `clean` `tracked` `ignored` `conflict`). Content search is
smart-case, and regexes match within a line, as in ripgrep.

## What grep covers

Text files up to 1 MB, and up to 10 MB inside git repos, minus what git
ignores. It leaves out lockfiles, JSON of 100 KB and up, minified or
bundled assets, linked git worktrees (their main checkout is indexed), and
build-output folders outside repos. To leave out more, list folders in `content-skip.txt`
beside the index (`~/Library/Application Support/FSearch/` on macOS, `$XDG_DATA_HOME/fsearch/`
— else `~/.local/share/fsearch/` — on Linux), one per line
(`~` and `#` comments work). Names are always indexed, and a grep inside
a file you name still reads it.

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
{"q": "fsearch main", "limit": 20, "offset": 20}
{"op": "grep", "pattern": "apply_dir", "in": "~/Developer", "context": 2}
{"q": "main.rs", "boost": true, "near": "~/Developer/app/src/lib.rs", "git": true}
{"op": "open", "path": "~/Developer/app/src/main.rs"}
{"op": "recent", "in": "~/Developer/app"}
```

`open` tells it what you opened; `"boost": true` ranks those higher for a
while, `near` favors files close to the one you're in, and `recent` lists
what you worked on lately. `"git": true` adds each hit's git status.

For coding agents, `fsearch mcp` is an MCP server on stdio with
`find_files`, `grep` and `multi_grep`.

Or link the crate in `Cargo.toml`:

```toml
[dependencies]
fsearch = { version = "0.2", package = "fsearch-crossplatform" }
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
  changes show up in about 15 ms, and a restart replays only what changed.
  On a first run, a search in a folder indexes that folder first.
- Names live in one mmap'd file, folder by folder, so `in:` is a range. A
  bitmap of each name's letters rules out most names before scoring.
- Content search is a trigram index plus a per-file filter, so a search
  only reads files likely to match. Identical files are indexed once, and
  deleted files' entries are compacted away. Matches are read fresh from
  disk; files edited since the last index sync are read directly.
- The daemon runs with malloc's space-efficient mode, so it gives memory
  back when idle.
