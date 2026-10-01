#!/usr/bin/env python3
"""navbench — honest benchmarks for navette vs Playwright+Chromium vs Lightpanda.

Local corpus only (100 generated pages served over loopback HTTP): nobody
crawls a real site 100 times. Metrics per ONEPAGER.md: install size, cold
start, navigate->read, act latency, RAM (process-tree peak), 100-page crawl.

Usage: python3 navbench.py [--pages 100] [--skip-playwright] [--skip-lightpanda]
Output: console table + BENCHMARKS.md next to this script's parent dir.
"""

import argparse
import functools
import http.server
import json
import os
import platform
import socketserver
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
import urllib.error

BENCH = os.path.dirname(os.path.abspath(__file__))
PROJ = os.path.dirname(BENCH)
CORPUS = os.path.join(BENCH, "corpus")
NAVETTE_BIN = os.path.join(PROJ, "target", "release", "navette")
PW_TARGET = os.path.join(BENCH, "pw_target.mjs")
HTTP_PORT = 8901
CORPUS_URL = f"http://127.0.0.1:{HTTP_PORT}"
N_PAGES = 100

# ---------------------------------------------------------------- corpus

def gen_corpus():
    os.makedirs(CORPUS, exist_ok=True)
    for i in range(N_PAGES):
        name = f"bench-{i:03d}"
        nxt = f"bench-{(i + 1) % N_PAGES:03d}"
        if i % 2 == 0:
            body = (f"<h1>Page {i}</h1><p>{name} static corpus page with a unique marker "
                    f"paragraph and some filler text to make innerText non-trivial.</p>"
                    f"<ul>{''.join(f'<li>item {j} of {name}</li>' for j in range(20))}</ul>")
        else:
            body = (f"<h1>Page {i}</h1><div id='dyn'></div><script>"
                    f"for(let j=0;j<100;j++){{const d=document.createElement('div');"
                    f"d.textContent='{name} js-built node '+j;"
                    f"document.getElementById('dyn').appendChild(d);}}</script>")
        body += (f"<a href='{nxt}.html'>next</a>"
                 f"<form id='f'><input id='name'><button id='go' type='button' "
                 f"onclick=\"document.getElementById('r').textContent='ok '+document.getElementById('name').value\">go</button></form>"
                 f"<div id='r'></div>")
        html = (f"<!DOCTYPE html><html><head><meta charset='utf-8'><title>{name}</title></head>"
                f"<body>{body}</body></html>")
        with open(os.path.join(CORPUS, name + ".html"), "w") as f:
            f.write(html)


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def serve_corpus():
    handler = functools.partial(QuietHandler, directory=CORPUS)
    socketserver.ThreadingTCPServer.allow_reuse_address = True
    httpd = socketserver.ThreadingTCPServer(("127.0.0.1", HTTP_PORT), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


# ---------------------------------------------------------------- helpers

def post(port, path, body, timeout=60):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}",
                                 data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def get(port, path, timeout=10):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=timeout) as r:
        return json.loads(r.read().decode())


def median(v):
    return statistics.median(v) if v else None


class RssSampler:
    """Samples the summed RSS (KB) of a pid's whole process tree."""

    def __init__(self, pid):
        self.pid = pid
        self.peak = 0
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _run(self):
        while not self.stop.is_set():
            try:
                out = subprocess.run(["ps", "-axo", "pid=,ppid=,rss="],
                                     capture_output=True, text=True).stdout
                info = {}
                children = {}
                for line in out.splitlines():
                    parts = line.split()
                    if len(parts) >= 3:
                        p, pp, rss = int(parts[0]), int(parts[1]), int(parts[2])
                        info[p] = rss
                        children.setdefault(pp, []).append(p)
                total, frontier, seen = 0, [self.pid], set()
                while frontier:
                    cur = frontier.pop()
                    if cur in seen:
                        continue
                    seen.add(cur)
                    total += info.get(cur, 0)
                    frontier.extend(children.get(cur, []))
                self.peak = max(self.peak, total)
            except Exception:
                pass
            self.stop.wait(0.2)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *a):
        self.stop.set()
        self.thread.join(timeout=1)


def run_node(mode, url, n, cdp=None, timeout=300):
    env = dict(os.environ)
    if cdp:
        env["CDP_URL"] = cdp
    p = subprocess.run(["node", PW_TARGET, mode, url, str(n)],
                       capture_output=True, text=True, env=env, timeout=timeout, cwd=BENCH)
    try:
        return json.loads(p.stdout.strip().splitlines()[-1])
    except Exception:
        return {"error": (p.stdout + p.stderr).strip()[:400]}


def du_kb(path):
    if not os.path.exists(path):
        return None
    r = subprocess.run(["du", "-sk", path], capture_output=True, text=True)
    try:
        return int(r.stdout.split()[0])
    except Exception:
        return None


def fmt_kb(kb):
    if kb is None:
        return "n/a"
    return f"{kb/1024:.1f} MB" if kb >= 1024 else f"{kb} KB"


# ---------------------------------------------------------------- targets

def bench_navette(pages_sample):
    res = {}

    # install size: the two binaries are the whole product (engine = OS)
    sizes = [du_kb(NAVETTE_BIN), du_kb(NAVETTE_BIN + "-mcp")]
    res["install_kb"] = sum(s for s in sizes if s)

    # cold start x3: spawn -> health -> first navigate+read
    colds = []
    for k in range(3):
        port = 8810 + k
        t0 = time.time()
        p = subprocess.Popen([NAVETTE_BIN, "--port", str(port)],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ready = False
        while time.time() - t0 < 20:
            try:
                if get(port, "/health", timeout=1).get("ok"):
                    ready = True
                    break
            except Exception:
                time.sleep(0.02)
        if ready:
            post(port, "/navigate", {"url": f"{CORPUS_URL}/bench-000.html", "with_content": True})
            colds.append((time.time() - t0) * 1000)
        p.kill()
    res["cold_ms"] = median(colds)

    # navigate->read on 12 pages (one session, alternating static/js)
    port = 8820
    p = subprocess.Popen([NAVETTE_BIN, "--port", str(port)],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.8)
    try:
        times = []
        for i in pages_sample:
            t0 = time.time()
            post(port, "/navigate", {"url": f"{CORPUS_URL}/bench-{i:03d}.html", "session": "bench",
                                     "with_content": True})
            times.append((time.time() - t0) * 1000)
        res["navread_ms"] = median(times)

        # act: type + click round-trips on the form page
        post(port, "/navigate", {"url": f"{CORPUS_URL}/bench-000.html", "session": "bench"})
        acts = []
        for i in range(12):
            t0 = time.time()
            post(port, "/type", {"selector": "#name", "value": f"agent {i}", "session": "bench"})
            post(port, "/click", {"selector": "#go", "session": "bench"})
            acts.append((time.time() - t0) * 1000)
        res["act_ms"] = median(acts)

        # crawl 100 pages with RSS sampling of the whole tree
        with RssSampler(p.pid) as s:
            t0 = time.time()
            for i in range(N_PAGES):
                post(port, "/navigate", {"url": f"{CORPUS_URL}/bench-{i:03d}.html", "session": "crawl",
                                         "with_content": True})
            res["crawl_ms"] = (time.time() - t0) * 1000
        res["peak_rss_kb"] = s.peak
    finally:
        p.kill()
    return res


def bench_playwright(pages_sample):
    res = {}
    cache = os.path.expanduser("~/Library/Caches/ms-playwright")
    res["browser_cache_kb"] = du_kb(cache)
    res["node_modules_kb"] = du_kb(os.path.join(BENCH, "node_modules"))
    if res["browser_cache_kb"] and res["node_modules_kb"]:
        res["install_kb"] = res["browser_cache_kb"] + res["node_modules_kb"]

    colds = []
    for _ in range(3):
        t0 = time.time()
        r = run_node("coldstart", f"{CORPUS_URL}/bench-000.html", 1, timeout=120)
        if "error" in r:
            res["error"] = r["error"]
            return res
        colds.append((time.time() - t0) * 1000)
    res["cold_ms"] = median(colds)

    r = run_node("navread", f"{CORPUS_URL}/bench-{{i}}.html", len(pages_sample))
    res["navread_ms"] = median(r.get("times", [])) if "times" in r else None
    r = run_node("act", f"{CORPUS_URL}/bench-000.html", 12)
    res["act_ms"] = median(r.get("times", [])) if "times" in r else None

    node_pid = None
    proc = subprocess.Popen(["node", PW_TARGET, "crawl", f"{CORPUS_URL}/bench-{{i}}.html", str(N_PAGES)],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=BENCH)
    node_pid = proc.pid
    with RssSampler(node_pid) as s:
        out, _ = proc.communicate(timeout=600)
    try:
        r = json.loads(out.strip().splitlines()[-1])
        res["crawl_ms"] = r.get("wallMs")
    except Exception:
        res["crawl_ms"] = None
    res["peak_rss_kb"] = s.peak
    return res


def ensure_lightpanda():
    bin_path = os.path.join(BENCH, "lightpanda")
    if os.path.exists(bin_path):
        return bin_path
    try:
        req = urllib.request.Request(
            "https://api.github.com/repos/lightpanda-io/browser/releases/latest",
            headers={"User-Agent": "navbench"})
        with urllib.request.urlopen(req, timeout=15) as r:
            rel = json.loads(r.read().decode())
        machine = "aarch64" if platform.machine() == "arm64" else "x86_64"
        asset = next((a for a in rel.get("assets", [])
                      if machine in a["name"] and "macos" in a["name"]), None)
        if not asset:
            return None
        path = os.path.join(BENCH, asset["name"])
        urllib.request.urlretrieve(asset["browser_download_url"], path)
        os.chmod(path, 0o755)
        return path
    except Exception as e:
        print(f"  [lightpanda] download failed: {e}", file=sys.stderr)
        return None


def bench_lightpanda(pages_sample):
    res = {}
    bin_path = ensure_lightpanda()
    if not bin_path:
        res["skipped"] = "binary not available"
        return res
    res["install_kb"] = du_kb(bin_path)
    proc = subprocess.Popen([bin_path, "serve", "--host", "127.0.0.1", "--port", "9333"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(1.5)
        cdp = "http://127.0.0.1:9333"
        r = run_node("coldstart", f"{CORPUS_URL}/bench-000.html", 1, cdp=cdp, timeout=120)
        if "error" in r:
            res["skipped"] = r["error"]
            return res
        res["cold_ms"] = r.get("navreadMs")
        r = run_node("navread", f"{CORPUS_URL}/bench-{{i}}.html", len(pages_sample), cdp=cdp)
        res["navread_ms"] = median(r.get("times", [])) if "times" in r else None
        r = run_node("act", f"{CORPUS_URL}/bench-000.html", 12, cdp=cdp)
        res["act_ms"] = median(r.get("times", [])) if "times" in r else None
        with RssSampler(proc.pid) as s:
            r = run_node("crawl", f"{CORPUS_URL}/bench-{{i}}.html", N_PAGES, cdp=cdp, timeout=600)
        res["crawl_ms"] = r.get("wallMs") if "wallMs" in r else None
        res["peak_rss_kb"] = s.peak
    finally:
        proc.kill()
    return res


# ---------------------------------------------------------------- report

ROWS = [
    ("Install size (what lands on disk)", "install_kb"),
    ("Cold start: launch -> first page read (ms, median of 3)", "cold_ms"),
    ("Navigate -> readable content (ms, median)", "navread_ms"),
    ("Act: type + click round-trip (ms, median)", "act_ms"),
    ("Peak RAM, whole process tree (MB)", "peak_rss_kb"),
    ("Crawl: 100 pages navigate+read (s)", "crawl_ms"),
]


def fmt(metric, v):
    if v is None:
        return "n/a"
    if metric == "install_kb":
        return f"{v/1024:.1f} MB"
    if metric == "peak_rss_kb":
        return f"{v/1024:.0f} MB"
    if metric == "crawl_ms":
        return f"{v/1000:.1f} s"
    return f"{v:.0f} ms"


def report(results, out_path):
    cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"],
                         capture_output=True, text=True).stdout.strip()
    ram_gb = None
    try:
        ram = subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True).stdout.strip()
        ram_gb = int(ram) / 2**30
    except Exception:
        pass
    lines = [
        "# navbench results",
        "",
        f"Date: {time.strftime('%Y-%m-%d %H:%M')} | Machine: {cpu} | "
        f"RAM: {ram_gb:.0f} GB | macOS {platform.mac_ver()[0]}",
        f"Corpus: {N_PAGES} local pages (50 static / 50 JS-built) served on loopback HTTP. "
        "No real site crawled. Medians; RAM = peak of the whole process tree.",
        "Note: navette `read` converts to markdown (more work than Playwright's innerText probe) "
        "and both Playwright numbers include its full client stack.",
        "",
        "| Metric | navette (system WebKit) | Playwright + Chromium | Lightpanda (CDP) |",
        "|---|---|---|---|",
    ]
    for label, key in ROWS:
        row = [label]
        for t in ("navette", "playwright", "lightpanda"):
            v = results.get(t, {}).get(key)
            note = results.get(t, {}).get("skipped")
            row.append(fmt(key, v) if v is not None else (f"skipped ({note[:40]})" if note else "n/a"))
        lines.append("| " + " | ".join(row) + " |")
    doc = "\n".join(lines) + "\n"
    with open(out_path, "w") as f:
        f.write(doc)
    print(doc)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-playwright", action="store_true")
    ap.add_argument("--skip-lightpanda", action="store_true")
    args = ap.parse_args()

    pages_sample = list(range(0, 12))
    print(f"[navbench] generating corpus ({N_PAGES} pages)...")
    gen_corpus()
    httpd = serve_corpus()
    time.sleep(0.3)

    results = {}
    if not os.path.exists(NAVETTE_BIN):
        print("[navbench] navette binary missing — run swift build -c release first")
        sys.exit(1)
    print("[navbench] target: navette")
    results["navette"] = bench_navette(pages_sample)
    print(results["navette"])

    if not args.skip_playwright:
        print("[navbench] target: playwright + chromium")
        results["playwright"] = bench_playwright(pages_sample)
        print(results["playwright"])

    if not args.skip_lightpanda:
        print("[navbench] target: lightpanda (best effort)")
        results["lightpanda"] = bench_lightpanda(pages_sample)
        print(results["lightpanda"])

    report(results, os.path.join(PROJ, "BENCHMARKS.md"))
    httpd.shutdown()


if __name__ == "__main__":
    main()
