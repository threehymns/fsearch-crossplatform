"""Chromium workload benchmark, Linux port: same workload as demo/vs_fff.py
(same seed, same queries over the daemon socket), fff half omitted.

usage: python3 bench_linux.py <folder> [out.json]
env: FSEARCH_BIN (default: ./target/release/fsearch),
     FSEARCH_HOME / FSEARCH_XDG (daemon's HOME/data dir; defaults: yours),
     SOCK (default: $XDG_DATA_HOME/fsearch/fsearch.sock).
Starts the daemon itself, waits for content to settle, times a restart,
then name queries (exact + 4 typo kinds) and literal greps.
Different hardware <-> read ratios, not absolutes. Daemon env is scrubbed
so a scratch HOME never hides the real toolchain (RUSTUP_HOME/CARGO_HOME
fall back to the invoking user's).
"""

import json, os, random, re, socket, statistics, subprocess, sys, time

ROOT = os.path.realpath(sys.argv[1])
OUT = sys.argv[2] if len(sys.argv) > 2 else "bench_chromium.json"
FSEARCH = os.environ.get("FSEARCH_BIN", "./target/release/fsearch")
HOME = os.environ.get("FSEARCH_HOME", os.path.expanduser("~"))
XDG = os.environ.get("FSEARCH_XDG", os.environ.get("XDG_DATA_HOME", ""))
SOCK = os.environ.get("SOCK") or os.path.join(
    XDG if XDG.startswith("/") else os.path.join(HOME, ".local/share"), "fsearch", "fsearch.sock")
# Restart probe: vs_fff.py uses `Makefile`; chromium has none, BUILD.gn sits
# at the top level instead. Any indexed query would do; this only times it.
PROBE = "BUILD.gn"
rng = random.Random(1)


def stats(ms):
    ms = sorted(ms)
    return {"p50": statistics.median(ms), "p90": ms[int(len(ms) * 0.9)], "n": len(ms)}


def rss_mb(pid):
    with open(f"/proc/{pid}/status") as f:
        m = re.search(r"VmRSS:\s+(\d+) kB", f.read())
    return int(m[1]) / 1024


# Targets: identical selection to vs_fff.py (fd's view), so the same tree
# yields the same queries.
files = {}
for p in subprocess.run(["fd", "-t", "f", "-0", ".", ROOT], capture_output=True, check=True).stdout.decode().split("\0")[:-1]:
    files.setdefault(os.path.basename(p), []).append(p)
pool = sorted(n for n, p in files.items() if len(p) == 1 and 6 <= len(n) <= 32 and n.isascii() and " " not in n
              and sum(c.isalpha() for c in n) >= 5)
targets = rng.sample(pool, 300)


def typo(name, kind):
    """One typo on a letter, never the first char (fsearch needs that one)."""
    s = list(name)
    letters = [i for i in range(1, len(s)) if s[i].isalpha()]
    p = rng.choice(letters)
    if kind == "swap":
        pairs = [i for i in letters if i + 1 < len(s) and s[i + 1].isalpha() and s[i] != s[i + 1]]
        if not pairs:
            return None
        p = rng.choice(pairs)
        s[p], s[p + 1] = s[p + 1], s[p]
    elif kind == "drop":
        del s[p]
    elif kind == "extra":
        s.insert(p, rng.choice("abcdefghijklmnopqrstuvwxyz"))
    else:
        s[p] = rng.choice([c for c in "abcdefghijklmnopqrstuvwxyz" if c != s[p].lower()])
    return "".join(s)


queries = [(t, t) for t in targets]
queries += [(q, t) for t in targets for k in ("swap", "drop", "extra", "subst") if (q := typo(t, k))]

# Content patterns: identical sampling to vs_fff.py.
srcs = [p for ps in files.values() for p in ps if p.endswith((".c", ".h", ".rs", ".ts", ".tsx", ".js", ".py", ".go", ".swift"))]
idents = set()
while len(idents) < 60:
    words = re.findall(r"\b[a-z][a-z0-9_]{9,30}\b", open(rng.choice(srcs), errors="ignore").read())
    if words:
        idents.add(rng.choice(words))
patterns = sorted(idents) + ["mutex_lock", "kmalloc", "EXPORT_SYMBOL_GPL", "spin_lock_irqsave", "struct device", "TODO", "return -EINVAL"]

REAL_HOME = os.environ.get("HOME", os.path.expanduser("~"))
env = dict(os.environ, HOME=HOME)
env.setdefault("RUSTUP_HOME", os.path.join(REAL_HOME, ".rustup"))
env.setdefault("CARGO_HOME", os.path.join(REAL_HOME, ".cargo"))
if "FSEARCH_XDG" in os.environ or XDG:
    env["XDG_DATA_HOME"] = XDG or os.environ.get("XDG_DATA_HOME", "")


def start_daemon():
    p = subprocess.Popen([FSEARCH, "serve"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(600):
        if os.path.exists(SOCK):
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(SOCK)
                s.close()
                return p
            except OSError:
                pass
        time.sleep(0.5)
    raise RuntimeError("daemon never opened its socket")


def stop_daemon(p):
    p.terminate()
    try:
        p.wait(timeout=10)
    except subprocess.TimeoutExpired:
        p.kill()
    for _ in range(100):
        if not os.path.exists(SOCK):
            return
        time.sleep(0.1)


daemon = start_daemon()
sock = socket.socket(socket.AF_UNIX)
sock.connect(SOCK)
conn = sock.makefile("rw")


def fs(req):
    t = time.perf_counter()
    conn.write(json.dumps(req) + "\n")
    conn.flush()
    r = json.loads(conn.readline())
    assert r.get("ok"), r
    return (time.perf_counter() - t) * 1000, r




def status():
    """status, waiting out 0.2's first-run scan: until the index is ready
    the daemon answers ok:false with an "indexing ..." error."""
    for _ in range(3600):
        t = time.perf_counter()
        conn.write(json.dumps({"op": "status"}) + "\n")
        conn.flush()
        r = json.loads(conn.readline())
        if r.get("ok"):
            return (time.perf_counter() - t) * 1000, r
        if "indexing" not in r.get("error", ""):
            raise AssertionError(r)
        time.sleep(1)
    raise RuntimeError("indexing never finished")


while status()[1]["content_pending"]:
    time.sleep(1)

# Restart from stopped: time the CLI's first answer, like vs_fff.py. The CLI
# restarts the daemon itself, so rediscover it afterwards.
stop_daemon(daemon)
t = time.perf_counter()
subprocess.run([FSEARCH, f"in:{ROOT}", PROBE], env=env, capture_output=True, check=True)
ready_s = time.perf_counter() - t
for _ in range(600):
    if os.path.exists(SOCK):
        break
    time.sleep(0.5)
out = subprocess.run(["pgrep", "-f", f"{FSEARCH} serve"], capture_output=True, text=True).stdout.split()
daemon_pid = int(out[0])
print(f"daemon pid after restart: {daemon_pid}", flush=True)
sock = socket.socket(socket.AF_UNIX)
sock.connect(SOCK)
conn = sock.makefile("rw")
while status()[1]["content_pending"]:
    time.sleep(0.5)


def fs_search(q):
    ms, r = fs({"q": f"in:{ROOT} {q}", "limit": 50})
    return ms, [h["path"] for h in r["hits"]]


def fs_grep(p, limit=50):
    ms, r = fs({"q": f"in:{ROOT}", "pattern": p, "op": "grep", "mode": "literal", "limit": limit, "per_file": 1, "budget_ms": 0})
    return ms, {f["path"] for f in r["files"]}


fs_search("warmup")
fs_grep("warmup")

res = {"folder_files": sum(len(v) for v in files.values()), "fsearch": {}}

times, found = [], {"exact": [0, 0], "typo": [0, 0]}
for q, target in queries:
    ms, hits = fs_search(q)
    times.append(ms)
    f = found["exact" if q == target else "typo"]
    f[0] += bool(hits) and os.path.basename(hits[0]) == target
    f[1] += 1
res["fsearch"]["name_ms"] = stats(times)
res["fsearch"]["exact_top1"] = found["exact"][0] / found["exact"][1]
res["fsearch"]["typo_top1"] = found["typo"][0] / found["typo"][1]

times = [fs_grep(p)[0] for p in patterns]
res["fsearch"]["grep_ms"] = stats(times)

res["fsearch"]["ready_s"] = ready_s
res["fsearch"]["memory_mb"] = rss_mb(daemon_pid)
res["fsearch"]["entries"] = fs({"op": "status"})[1]["entries"]
res["patterns"], res["queries"] = len(patterns), len(queries)
import signal
os.kill(daemon_pid, signal.SIGTERM)
for _ in range(100):
    try:
        os.kill(daemon_pid, 0)
    except ProcessLookupError:
        break
    time.sleep(0.1)
json.dump(res, open(OUT, "w"), indent=1)
print(json.dumps(res, indent=1))
