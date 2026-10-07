---
title: ~1 MB to orbit: your agent's browser ships with your OS
published: true
description: A single Rust binary that drives the WebView your OS already ships — 0.6–1.2 MB, no Chromium, MCP-native.
tags: showdev, rust, ai, webdev
---

![navette — the browser for agents](https://raw.githubusercontent.com/slabbdev/navette/main/assets/banner.png)

**Houston, we deleted Chromium.**

Every AI agent that touches the web today drags the same luggage: **headless Chromium**.

Playwright downloads ~400 MB of browser on every machine your agent touches. Cloud browser APIs (Browserbase, Steel, Hyperbrowser) rent you Chrome by the subscription — and your pages leave your machine. And all of this to do what an agent actually needs: *open a page, read it, take a screenshot, click a button, fill a form.*

Meanwhile, your computer **already ships a full browser engine**. macOS has WebKit. Windows has WebView2 — which is Chromium, preinstalled. Linux has WebKitGTK.

So I built **navette** (French for *shuttle*): a single Rust binary that drives the WebView your OS already ships, and speaks MCP — the protocol agents already speak.

**≤ 1.2 MB installed. No Chromium. No download.**

## What it is

One binary, three modes:

```sh
navette serve --port 8765     # HTTP API on loopback
navette mcp                   # MCP stdio server for agent hosts
navette install-daemon        # resident: warm from login
```

Sixteen MCP tools as of v1.4. The core loop: `navigate`, `read`, `screenshot`, `click`, `type`, `evaluate`, `wait`, `sessions`, `session_close` — since joined by scroll, file upload, viewport control, in-page dialog handling, and session state export/import. The screenshot comes back as MCP image content — your agent literally **sees** the page. Full tool list in the repo.

Under the hood on macOS: a WKWebView per session living in a **ghost window** (real pixels, parked off-screen so it renders but nobody sees it). Navigation completion is event-driven — I implemented `WKNavigationDelegate` in Rust with raw `objc2` message sends, and the content extraction runs *inside* the `didFinish` callback, so a warm navigate + full markdown read costs **8 ms**.

## The numbers

Same machine (M1, 8 GB), same corpus (100 local pages + 20 real URLs), reproducible with one command (`python3 bench/navbench.py`). macOS numbers; cross-engine medians and p95s run in CI (see the repo):

| Metric | navette | Playwright + Chromium | Lightpanda |
|---|---|---|---|
| Install size (per-OS binary) | **0.6–1.2 MB** | 216 MB | 80 MB |
| Cold start (resident daemon) | **24–37 ms** | n/a | ~320 ms boot / launch |
| Cold start (fresh process) | 531–984 ms | 934–1539 ms | ~320 ms |
| Navigate → readable content | **8–11 ms** | 12–33 ms | 3–16 ms |
| Act (type + click) | **1–2 ms** | 33 ms | 18–204 ms |
| Peak RAM (process tree) | 79 MB | 536 MB | 41 MB |
| Crawl 100 pages | **0.9–2.8 s** | 1.8–4.0 s | 0.9–2.6 s |
| Real-web success (20 URLs) | **95–100%** | — | 85% |

Two honest disclosures, because benchmark posts die without them:

1. **Lightpanda is the fastest page-*reader*** (3.3 ms through a zero-bias raw-CDP probe — I measured it through a fair minimal client, not just through Playwright). It parses a partial DOM and cannot render, screenshot, or act. If your agent only reads static pages, use it — or plain fetch + readability.
2. **The two rows Lightpanda wins (fresh boot, peak RAM) are the price of rendering.** Three WebKit helper processes and a backing store are what buy screenshots and full fidelity. That tax *is* the product.

The claim I'll defend: **nobody else combines ≤ 1.2 MB + 1 ms actions + a 0.9 s hundred-page crawl + full WebKit-class rendering + a resident 24 ms mode.**

## The demo that sold me on it

I pointed the resident daemon at a login form and a research loop and let an agent drive through MCP only:

- **Act 1 — authenticate**: type credentials, click, verify the session (`"You logged into a secure area!"`), screenshot the authenticated page.
- **Act 2 — research**: DuckDuckGo → extract results → open one (it was dead — moved on) → read the article → screenshot.

Full walkthrough with the verbatim tool calls and screenshots is in [`demo/JOURNEY.md`](https://github.com/slabbdev/navette/blob/main/demo/JOURNEY.md).

## Try it

```sh
brew install slabbdev/navette/navette     # macOS arm64
cargo install navette-browser             # any platform
docker run -i --rm ghcr.io/slabbdev/navette navette mcp   # container, stdio MCP

# or from source:
cargo build --release
./target/release/navette serve --port 8765

# or let your agent host own the lifecycle:
./target/release/navette mcp
```

Register it in any MCP client (ZCode example):

```json
{ "mcp": { "servers": { "navette": {
    "command": "/path/to/navette", "args": ["mcp"] } } } }
```

Then just talk to your agent: *"open this dashboard, check if the deploy banner is there, screenshot it for me."*

## The honest limits

- **Three engines, one fully hardened.** Windows (WebView2) and Linux (WebKitGTK) reached platform parity in v1.4 — screenshots, cookie state, viewport, resident daemon, CI-verified on all three OSes — but macOS remains the benchmark-carrying backend. The engine strategy is written down: system-first, official embed (WebView2 Fixed Version) or distro packages as fallback. Never vendor a browser the way Playwright does.
- **The automation long tail is still long.** Still missing, stated plainly: OS-level keyboard/mouse input, request/response network interception, OS file-dialog automation — Playwright has twenty years of API surface. The core agent loop (navigate, read, see, act, wait, sessions, state) is complete; the tail is the roadmap.
- **Fresh-process cold start can't beat a blind engine.** A full WebKit spawns three helper processes; Lightpanda spawns none because it renders nothing. That's why navette ships a resident mode: warm from login, 24 ms forever.

## Why this matters

Headless Chromium is the Postgres of web automation. navette is trying to be the embedded SQLite — the browser agents *bring with them*, for the fastest-growing half of the agent world: agents that run **locally**, on machines that already have an engine.

Apple shipped a Safari MCP server for coding agents this year. The thesis is being validated from above. navette is it from below: tiny, open, cross-platform.

## Changelog

- **v1.2.0 — Oct 2**: Windows (WebView2) and Linux (WebKitGTK) backends aboard; `state_export` / `state_import` — cookies and storage out of the browser and back in, the Playwright `storageState` equivalent. 11 tools. [Release](https://github.com/slabbdev/navette/releases/tag/v1.2.0)
- **v1.2.1 — Oct 3**: CI green on all three OSes (same navigate→read smoke). Getting there surfaced five real root causes — fold-result routing, URL-matched completion, a missing D-Bus session bus in headless Linux, WebView2's `file://` stall — each fixed and proven with a breadcrumb or backtrace. Boot-time pre-warm parity: first navigate on a cold CI VM went 38 s → 83 ms. [Release](https://github.com/slabbdev/navette/releases/tag/v1.2.1)
- **v1.3.0 & v1.4.0 — Oct 4**: full platform parity (native screenshots per engine, cookie round-trip, viewport control, resident daemon — CI verifies a valid PNG and a cookie round-trip on all three OSes); JS dialogs auto-handled in-page; the agent loop completed with file upload (in-memory DataTransfer, no OS dialog) and scroll — **16 tools**; fixed an eval-wrapper bug where every `/evaluate` and `/read` outside the fold path returned `undefined` on Windows/Linux (CI now tests it); one-command install `cargo install navette-browser`. [Release](https://github.com/slabbdev/navette/releases/tag/v1.4.0)
- **v1.5.0 — Oct 5**: the idle watchdog — `serve --idle-release 15` drops WebKit sessions after 15 minutes without requests while the daemon stays resident; the next request re-warms on demand (38 s cold → 83 ms warm on the CI VM). Plus the tollbooth bench harness: ~200 real URLs × 5 categories, measuring robots.txt AI clauses, llms.txt adoption and the agent path itself — the walled-web dataset is committed to the repo (its own post soon). [Release](https://github.com/slabbdev/navette/releases/tag/v1.5.0)
- **v1.6.0 — Oct 7**: the operator release — `--token` (Bearer auth on the HTTP API, MCP hosts attach with `NAVETTE_TOKEN`), `--proxy` (HTTP CONNECT / SOCKS5), `--user-agent` (per-serve override); linux-arm64 joins the release assets; cross-engine medians (warm navigate+read 5.7–23.4 ms on all three OSes) landed in BENCHMARKS.md. [Release](https://github.com/slabbdev/navette/releases/tag/v1.6.0)
- **Sizing, honestly**: the title's 594 KB was v1.0. macOS arm64 is 658 KB today, Windows x64 / Linux x64 ~1.2 MB — still ~180× smaller than Playwright's browser download.

Repo, benchmarks and the reproducible harness: **https://github.com/slabbdev/navette**

If you run agents locally, I'd genuinely love your feedback — especially the failure cases.
