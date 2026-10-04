import json, os, statistics, subprocess, sys, time, urllib.request

PORT = 8765
BASE = f"http://127.0.0.1:{PORT}"
LABEL = os.environ.get("BENCH_LABEL", "unknown")


def req(path, body=None):
    r = urllib.request.Request(
        BASE + path,
        method="POST" if body else "GET",
        data=json.dumps(body).encode() if body else None,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(r, timeout=60) as resp:
        return json.loads(resp.read())


# 30 local pages with varied content
os.makedirs("corpus", exist_ok=True)
for i in range(30):
    paras = "\n".join(
        f"<p>Paragraph {j} of page {i} with agent-relevant filler text.</p>" for j in range(12)
    )
    open(
        f"corpus/page{i}.html", "w"
    ).write(
        f"<!DOCTYPE html><html><head><title>Bench page {i}</title></head>"
        f"<body><h1>Page {i}</h1>{paras}<a href='page{(i+1)%30}.html'>next</a></body></html>"
    )

httpd = subprocess.Popen(
    [sys.executable, "-m", "http.server", "8901", "-d", "corpus"],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)

exe = "./target/release/navette" + (".exe" if os.name == "nt" else "")
serve = subprocess.Popen([exe, "serve", "--port", str(PORT)])
for _ in range(30):
    try:
        req("/health")
        break
    except Exception:
        time.sleep(0.5)

# warm-up (session creation, JIT caches)
req("/navigate", {"url": "http://127.0.0.1:8901/page0.html", "with_content": True})

nav_times, read_times, shot_times = [], [], []
for i in range(30):
    t0 = time.perf_counter()
    req("/navigate", {"url": f"http://127.0.0.1:8901/page{i}.html", "with_content": True})
    nav_times.append((time.perf_counter() - t0) * 1000)
    t0 = time.perf_counter()
    req("/read", {"format": "text"})
    read_times.append((time.perf_counter() - t0) * 1000)
    t0 = time.perf_counter()
    r = urllib.request.Request(BASE + "/screenshot", method="POST", data=b"")
    with urllib.request.urlopen(r, timeout=60) as resp:
        len(resp.read())
    shot_times.append((time.perf_counter() - t0) * 1000)


def stats(xs):
    p95 = sorted(xs)[max(0, int(len(xs) * 0.95) - 1)]
    return f"median {statistics.median(xs):.1f} ms · p95 {p95:.1f} ms"


row = f"| {LABEL} | {stats(nav_times)} | {stats(read_times)} | {stats(shot_times)} |"
print(row)
out = os.environ.get("GITHUB_STEP_SUMMARY")
if out:
    if not os.path.exists(out) or os.path.getsize(out) == 0:
        with open(out, "a") as f:
            f.write("| Engine | navigate+read | /read | /screenshot |\n|---|---|---|---|\n")
    with open(out, "a") as f:
        f.write(row + "\n")

serve.terminate()
httpd.terminate()
