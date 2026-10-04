---
title: 594 KB to orbit: a browser for AI agents with no Chromium attached
description: Houston, we deleted Chromium: your agent's browser is 594 KB and ships with your OS. Full WebKit, MCP-native, resident at 24 ms.
published: false
tags: showdev, rust, ai, webdev
---

**Houston, we deleted Chromium.**

Every AI agent that touches the web today drags the same luggage: **headless Chromium**.

Playwright downloads ~400 MB of browser on every machine your agent touches. Cloud browser APIs (Browserbase, Steel, Hyperbrowser) rent you Chrome by the subscription — and your pages leave your machine. And all of this to do what an agent actually needs: *open a page, read it, take a screenshot, click a button, fill a form.*

Meanwhile, your computer **already ships a full browser engine**. macOS has WebKit. Windows has WebView2 — which is Chromium, preinstalled. Linux has WebKitGTK.

So I built **navette** (French for *shuttle*): a single Rust binary that drives the WebView your OS already ships, and speaks MCP — the protocol agents already speak.

**594 KB installed. No Chromium. No download.**

## What it is

One binary, three modes:

```sh
navette serve --port 8765     # HTTP API on loopback
navette mcp                   # MCP stdio server for agent hosts
navette install-daemon        # resident: warm from login
```

Nine MCP tools: `navigate`, `read`, `screenshot`, `click`, `type`, `evaluate`, `wait`, `sessions`, `session_close`. The screenshot comes back as MCP image content — your agent literally **sees** the page.

Under the hood: a WKWebView per session living in a **ghost window** (real pixels, parked off-screen so it renders but nobody sees it). Navigation completion is event-driven — I implemented `WKNavigationDelegate` in Rust with raw `objc2` message sends, and the content extraction runs *inside* the `didFinish` callback, so a warm navigate + full markdown read costs **8 ms**.

## The numbers

Same machine (M1, 8 GB), same corpus (100 local pages + 20 real URLs), reproducible with one command (`python3 navbench.py`):

| Metric | navette | Playwright + Chromium | Lightpanda |
|---|---|---|---|
| Install size | **0.6 MB** | 216 MB | 80 MB |
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

The claim I'll defend: **nobody else combines 0.6 MB + 1 ms actions + a 0.9 s hundred-page crawl + full WebKit rendering + a resident 24 ms mode.**

## The demo that sold me on it

I pointed the resident daemon at a login form and a research loop and let an agent drive through MCP only:

- **Act 1 — authenticate**: type credentials, click, verify the session (`"You logged into a secure area!"`), screenshot the authenticated page.
- **Act 2 — research**: DuckDuckGo → extract results → open one (it was dead — moved on) → read the article → screenshot.

Full walkthrough with the verbatim tool calls and screenshots is in [`demo/JOURNEY.md`](https://github.com/slabbdev/navette/blob/main/demo/JOURNEY.md).

## Try it

```sh
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

- **macOS today.** Windows (WebView2) and Linux (WebKitGTK/WPE) are the next backends — the 8 primitives are engine-agnostic by design, and the engine strategy is written down: system-first, official embed (WebView2 Fixed Version) or distro packages as fallback. Never vendor a browser the way Playwright does.
- **The automation long tail is missing.** Network interception, file uploads, hover, dialogs — Playwright has twenty years of API surface. The core agent loop (navigate, read, see, act, wait, sessions) is complete; the tail is the roadmap.
- **Fresh-process cold start can't beat a blind engine.** A full WebKit spawns three helper processes; Lightpanda spawns none because it renders nothing. That's why navette ships a resident mode: warm from login, 24 ms forever.

## Why this matters

Headless Chromium is the Postgres of web automation. navette is trying to be the embedded SQLite — the browser agents *bring with them*, for the fastest-growing half of the agent world: agents that run **locally**, on machines that already have an engine.

Apple shipped a Safari MCP server for coding agents this year. The thesis is being validated from above. navette is it from below: tiny, open, cross-platform-bound.

Repo, benchmarks and the reproducible harness: **https://github.com/slabbdev/navette**

If you run agents locally, I'd genuinely love your feedback — especially the failure cases.

---

**Update — v1.4.0 (Oct 4):**

- The agent loop is complete on every OS: **file upload** (in-memory content through a page-side DataTransfer — no OS dialog, identical JS on all three engines) and **scroll** join the API. **16 MCP tools** now. CI proves the upload round-trip on macOS, Windows and Linux: upload a file, let the page's own JS read it, assert the result.
- Worth the release on its own: the eval wrapper on Windows/Linux silently discarded its value — every `/evaluate` and `/read` outside the fold path returned `undefined`. Fixed, and CI tests it explicitly now.
- **Release binaries attached** (macOS arm64 658 KB, Windows x64 / Linux x64 ~1.2 MB — still ~180× smaller than Playwright's download): https://github.com/slabbdev/navette/releases/tag/v1.4.0
- Stated plainly, still missing: OS-level keyboard/mouse input (synthetic events today), request/response network interception, OS file-dialog automation.
