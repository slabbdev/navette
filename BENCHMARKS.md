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
