"""p50/p90/p99 (ms) per category from examples/rig.rs outputs: each query's
median over its rounds, then percentiles over the queries; typing runs are
divided by their keystrokes. With --diff A B: the queries whose result
fingerprints differ.

usage: rigstats.py <rig out.json>... | rigstats.py --diff <a.json> <b.json>"""
import json, sys
from collections import defaultdict

if sys.argv[1] == "--diff":
    a, b = json.load(open(sys.argv[2])), json.load(open(sys.argv[3]))
    d = [k for k in a if isinstance(a[k], dict) and k in b and a[k]["digest"] != b[k]["digest"]]
    cats = defaultdict(int)
    for k in d:
        cats[a[k]["cat"]] += 1
    print(len(d), "differ:", dict(cats))
    for k in d:
        print("  ", a[k]["cat"], k.split(" ", 1)[1])
    sys.exit()
cats = defaultdict(list)
for f in sys.argv[1:]:
    for k, v in json.load(open(f)).items():
        if isinstance(v, dict):
            t = sorted(v["ms"])
            m = t[len(t) // 2]
            if v["kind"] == "typing":
                m /= len(k.split(" ")[-1])
            cats[v["kind"] + " " + v["cat"]].append(m)
            cats[v["kind"] + " ALL"].append(m)
pct = lambda x, p: sorted(x)[min(len(x) - 1, int(len(x) * p))]
for c, x in sorted(cats.items()):
    print(f"{c:22} n={len(x):4}  p50 {pct(x, .5):7.3f}  p90 {pct(x, .9):7.3f}  p99 {pct(x, .99):7.3f}  max {max(x):7.3f}")
