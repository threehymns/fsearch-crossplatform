#!/bin/sh
# Release build optimized with a profile of fsearch's own searches (PGO):
# name searches run 10-35% faster. Trains on a copy of your index with
# queries sampled from it (examples/rig.rs), then builds target/release.
# Needs: rustup component add llvm-tools
#
# Portable by design: the profile only reorders code and branch hints, so a
# PGO binary runs on any machine of its target arch. That is what ships in
# releases (tarballs, .deb, cargo-binstall).
#
# For one more ~8% on a machine you build for directly, add native CPU
# tuning to both cargo lines below (it bakes the build host's instructions
# into the binary, so never ship that to other machines):
#   RUSTFLAGS="-Cprofile-generate=$tmp/prof -C target-cpu=native" ...
#   RUSTFLAGS="-Cprofile-use=$tmp/fsearch.profdata -C target-cpu=native" ...
# (Upstream enables target-cpu=native by default, but only for
# aarch64-apple-darwin in .cargo/config.toml, where every chip shares the
# baseline. x86_64 spans fifteen years of feature levels, so Linux releases
# stay portable and native stays an explicit self-build opt-in.)
set -e
profdata=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata 2>/dev/null | head -1)
[ -x "$profdata" ] || { echo "pgo.sh: needs llvm-profdata: rustup component add llvm-tools" >&2; exit 1; }
if [ "$(uname)" = "Darwin" ]; then
    data="$HOME/Library/Application Support/FSearch"
else
    # Same resolution as Engine::default_dir: XDG requires absolute.
    case "${XDG_DATA_HOME:-}" in
        /*) data="$XDG_DATA_HOME/fsearch" ;;
        *) data="$HOME/.local/share/fsearch" ;;
    esac
fi
[ -f "$data/index.bin" ] || { echo "pgo.sh: no index yet; run fsearch once first" >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/snap"
cp -c "$data/index.bin" "$tmp/snap/" 2>/dev/null || cp "$data/index.bin" "$tmp/snap/"
cp -cR "$data/content" "$tmp/snap/" 2>/dev/null || cp -R "$data/content" "$tmp/snap/"
RUSTFLAGS="-Cprofile-generate=$tmp/prof" cargo build --release --example rig --target-dir "$tmp/target"
"$tmp/target/release/examples/rig" gen "$tmp/snap" "$tmp/corpus.json"
"$tmp/target/release/examples/rig" run "$tmp/snap" "$tmp/corpus.json" "$tmp/out.json" >/dev/null
"$profdata" merge -o "$tmp/fsearch.profdata" "$tmp/prof"
RUSTFLAGS="-Cprofile-use=$tmp/fsearch.profdata" cargo build --release
