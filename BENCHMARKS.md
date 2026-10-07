# navbench results

Date: 2026-10-01 14:56 | Machine: Apple M1 | RAM: 8 GB | macOS 26.7.1
Corpus: 100 local pages (50 static / 50 JS-built) served on loopback HTTP. No real site crawled. Medians; RAM = peak of the whole process tree.
Note: navette `read` converts to markdown (more work than Playwright's innerText probe) and both Playwright numbers include its full client stack.

| Metric | navette (system WebKit) | Playwright + Chromium | Lightpanda (CDP) |
|---|---|---|---|
| Install size (what lands on disk) | 0.6 MB | 215.8 MB | 96.1 MB |
| Cold start: launch -> first page read (ms, median of 3) | 669 ms | 934 ms | 33 ms |
| Navigate -> readable content (ms, median) | 11 ms | 12 ms | 12 ms |
| Act: type + click round-trip (ms, median) | 2 ms | 33 ms | 18 ms |
| Peak RAM, whole process tree (MB) | 79 MB | 599 MB | 41 MB |
| Crawl: 100 pages navigate+read (s) | 1.5 s | 1.8 s | 0.9 s |


## Cross-engine medians (CI runners, v1.6.0)

30-page local corpus, warm loop, `bench/bench.py` via the `bench` workflow (run of 2026-10-07). Shared CI machines — treat as relative, not absolute:

| Engine | navigate+read | /read | /screenshot |
|---|---|---|---|
| macOS arm64 (WKWebView) | median 15.0 ms · p95 26.4 ms | median 0.8 ms | median 22.8 ms |
| Linux x64 (WebKitGTK) | median 5.7 ms · p95 6.0 ms | median 0.9 ms | median 2018 ms |
| Windows x64 (WebView2) | median 23.4 ms · p95 30.7 ms | median 1.7 ms | median 45.2 ms |

Honest reading: warm navigate+read is single-digit-to-low-tens of milliseconds everywhere. The Linux screenshot path (on-screen move + GTK pump + X11 capture + restore) costs ~2 s — the price of capturing an engine wry doesn't let us snapshot directly; Windows (`PrintWindow`) and macOS (`takeSnapshot`) stay in the tens of milliseconds. Re-run with `gh workflow run bench`.
