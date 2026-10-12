"""Name-search quality: sample files under a git checkout, make the queries
people type for them, and score where a build ranks the file meant.

  quality.py gen <root> <n> <out.json>               queries (fixed seed)
  quality.py report <queries.json> <name=ranks.json>...
  quality.py diff <queries.json> <a.json> <b.json> [kinds] [n] [worse|better]   rank changes
  quality.py fails <queries.json> <ranks.json> [kinds] [n]        queries not ranked first
  quality.py rig <out.json> <queries.json>...         whole-disk speed corpus for examples/rig.rs

Targets are tracked files whose name is unique there;
`dup` targets share their name but not their folder/name. A query wants the
target, and for stem queries any file with the same stem (foo.c, foo.h).
fsearch's ranks come from examples/quality.rs.
"""

import json, os, random, re, subprocess, sys
from collections import defaultdict

LETTERS = "abcdefghijklmnopqrstuvwxyz"


def base(f):
    return f.rsplit("/", 1)[-1]


def stem(name):
    dot = name.rfind(".")
    return name[:dot] if dot > 0 else name


def words(s):
    out = []
    for p in re.split(r"[_\-. ]+", s):
        out += re.findall(r"[A-Z]+(?![a-z])|[A-Z]?[a-z]+|[0-9]+", p)
    return out


def edit(s, p, kind, rng):
    """One edit at position p: wrong letter, extra letter, missing letter, or swapped with the next."""
    c = s[p].lower()
    if kind == "subst":
        return s[:p] + rng.choice([x for x in LETTERS if x != c]) + s[p + 1 :]
    if kind == "extra":
        return s[:p] + rng.choice(LETTERS) + s[p:]
    if kind == "drop":
        return s[:p] + s[p + 1 :]
    return s[:p] + s[p + 1] + s[p] + s[p + 2 :]


def typo(s, p, rng):
    kinds = ["subst", "extra", "drop"] + (["swap"] if p + 1 < len(s) and s[p + 1].isalpha() and s[p + 1].lower() != s[p].lower() else [])
    return edit(s, p, rng.choice(kinds), rng)


def gen(root, n, out):
    rng = random.Random(7)
    ls = subprocess.run(["git", "-C", root, "ls-files", "-z"], capture_output=True, check=True).stdout
    files = [f for f in ls.decode(errors="replace").split("\0")[:-1] if os.path.isfile(os.path.join(root, f)) and not os.path.islink(os.path.join(root, f))]
    by_name, by_stem, by_pair = defaultdict(list), defaultdict(list), defaultdict(list)
    for f in files:
        by_name[base(f).lower()].append(f)
        by_stem[stem(base(f)).lower()].append(f)
        by_pair["/".join(f.split("/")[-2:]).lower()].append(f)
    ok = lambda f: re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", base(f)) and sum(c.isalpha() for c in stem(base(f))) >= 3 and len(base(f)) <= 40
    uniq = sorted(f for f in files if len(by_name[base(f).lower()]) == 1 and ok(f))
    dup = sorted(f for f in files if len(by_name[base(f).lower()]) > 1 and len(by_pair["/".join(f.split("/")[-2:]).lower()]) == 1 and "/" in f and ok(f))
    qs = []

    def add(kind, q, want, f):
        if q and q not in (x["q"] for x in qs[-12:]):
            qs.append({"kind": kind, "q": q, "want": sorted(set(want)), "target": f})

    for f in rng.sample(uniq, n):
        name = base(f)
        s = stem(name)
        sibs = by_stem[s.lower()]
        if len(sibs) > 4:
            sibs = [f]
        dirs = f.split("/")[:-1]
        letters = [i for i, c in enumerate(s) if c.isalpha()]
        w = words(s)
        add("exact", name, [f], f)
        if s != name:
            add("stem", s, sibs, f)
        if len(letters) >= 3:
            band = "short" if len(s) <= 4 else "long" if len(s) >= 8 else ""
            add("typo-first", typo(s, letters[0], rng), sibs, f)
            add("typo-mid", typo(s, rng.choice(letters[1:-1]), rng), sibs, f)
            add("typo-last", typo(s, letters[-1], rng), sibs, f)
            if band:
                add(f"typo-{band}", typo(s, rng.choice(letters), rng), sibs, f)
            if len(s) >= 8:
                a, b = sorted(rng.sample(letters, 2))
                add("typo2", typo(typo(s, b, rng), a, rng), sibs, f)
        if len(s) >= 6:
            add("partial", s[: (len(s) * 3 + 4) // 5], sibs, f)
        if len(w) >= 2:
            add("words", " ".join(x.lower() for x in w), sibs, f)
            add("abbrev", "".join(x[:3].lower() for x in w), sibs, f)
            if re.search(r"[_\-. ]", s):
                add("concat", "".join(x.lower() for x in w), sibs, f)
                add("camel", "".join(x[0].upper() + x[1:].lower() for x in w), sibs, f)
            else:
                add("snake", "_".join(x.lower() for x in w), sibs, f)
        if dirs:
            add("path", f"{dirs[-1]}/{name}", [f], f)
            add("path-stem", f"{dirs[-1]}/{s}", sibs, f)
            add("dir-word", f"{rng.choice(dirs)} {rng.choice(w).lower() if w else s}", sibs, f)
            if len(dirs) >= 2:
                add("path-frag", f"{rng.choice(dirs[:-1])}/{s}", sibs, f)
            d = dirs[-1]
            dl = [i for i, c in enumerate(d) if c.isalpha()]
            if len(d) >= 5 and dl:
                add("path-typo", f"{typo(d, rng.choice(dl), rng)}/{s}", sibs, f)
            add("fullpath", f, [f], f)
    for f in rng.sample(dup, n // 2):
        dirs, name = f.split("/")[:-1], base(f)
        add("dup-path", f"{dirs[-1]}/{name}", [f], f)
        add("dup-words", f"{dirs[-1]} {stem(name)}", [f], f)
    json.dump(qs, open(out, "w"), indent=0)
    kinds = defaultdict(int)
    for q in qs:
        kinds[q["kind"]] += 1
    print(len(files), "files,", len(qs), "queries:", dict(kinds))


def report(queries, runs):
    qs = json.load(open(queries))
    named = [(r.split("=", 1)[0], json.load(open(r.split("=", 1)[1]))) for r in runs]
    kinds = sorted({q["kind"] for q in qs}, key=lambda k: [q["kind"] for q in qs].index(k))
    head = f"{'kind':11} {'n':>4}" + "".join(f"  {n + ' top1':>11} {'top5':>5} {'MRR':>5}" for n, _ in named)
    print(head)
    for k in kinds + ["ALL"]:
        idx = [i for i, q in enumerate(qs) if k in (q["kind"], "ALL")]
        line = f"{k:11} {len(idx):4}"
        for n, r in named:
            ranks = [r[i]["rank"] for i in idx]
            t1 = sum(x == 1 for x in ranks) / len(ranks)
            t5 = sum(1 <= x <= 5 for x in ranks) / len(ranks)
            mrr = sum(1 / x for x in ranks if x) / len(ranks)
            line += f"  {t1 * 100:11.1f} {t5 * 100:5.1f} {mrr:5.3f}"
        print(line)


def fails(queries, a, kinds=None, n=20):
    """Queries a run did not rank first, with what it ranked first."""
    qs, a = json.load(open(queries)), json.load(open(a))
    kinds = kinds.split(",") if kinds else None
    shown = 0
    for q, x in zip(qs, a):
        if x["rank"] != 1 and (not kinds or q["kind"] in kinds) and shown < int(n):
            shown += 1
            print(f"{q['kind']:10} {q['q']!r:34} {x['rank']:>3}  want={q['target']}  top={x['top'][:2]}")


def diff(queries, a, b, kinds=None, n=20, only=None):
    """Queries whose rank changed (only "worse" or "better" ones if asked)."""
    qs, a, b = json.load(open(queries)), json.load(open(a)), json.load(open(b))
    kinds = kinds.split(",") if kinds else None
    better = worse = shown = 0
    for q, x, y in zip(qs, a, b):
        rx, ry = x["rank"] or 999, y["rank"] or 999
        if rx == ry or (kinds and q["kind"] not in kinds):
            continue
        better, worse = better + (ry < rx), worse + (ry > rx)
        if shown < int(n) and only in (None, "worse" if ry > rx else "better"):
            shown += 1
            print(f"{q['kind']:10} {q['q']!r:34} {x['rank']:>3} -> {y['rank']:>3}  want={base(q['target'])}  top={[base(t) for t in y['top'][:2]]}")
    print(f"better {better} worse {worse}")


def rig(out, sets):
    """Each kind's first 12 queries per set, and typing runs: 8 stems per
    set keystroke by keystroke, every other one with a typo. A set given as
    <queries.json>=<root> is scoped to root (in:root)."""
    rng = random.Random(3)
    names, typing = [], []
    for f in sets:
        f, _, root = f.partition("=")
        scope = f"in:{root} " if root else ""
        qs = json.load(open(f))
        seen = defaultdict(int)
        for q in qs:
            if seen[q["kind"]] < 12:
                seen[q["kind"]] += 1
                names.append({"cat": q["kind"], "q": scope + q["q"]})
        stems = [q["q"] for q in qs if q["kind"] == "stem" and len(q["q"]) >= 8][:8]
        for i, s in enumerate(stems):
            if i % 2:
                p = rng.randrange(1, len(s) - 1)
                s = typo(s, p, rng) if s[p].isalpha() else s
            typing.append([scope + s[:n] for n in range(1, len(s) + 1)])
    json.dump({"names": names, "typing": typing, "grep": []}, open(out, "w"), indent=0)
    print(len(names), "name queries,", len(typing), "typing runs")


if __name__ == "__main__":
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "gen":
        gen(os.path.realpath(args[0]), int(args[1]), args[2])
    elif cmd == "diff":
        diff(*args)
    elif cmd == "fails":
        fails(*args)
    elif cmd == "rig":
        rig(args[0], args[1:])
    else:
        report(args[0], args[1:])
