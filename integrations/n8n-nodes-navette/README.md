# n8n-nodes-navette

**navette in your n8n workflows** — the browser for agents, as a community
node. Navigate, read, screenshot, click, type and evaluate a **real system
WebView** (WebKit on macOS, WebView2 on Windows, WebKitGTK on Linux) driven
by the 1 MB [navette](https://github.com/slabbdev/navette) daemon — no
Chromium download, no 557 MB browser cache in your CI or container.

## Two ways to use it

**Beginner — build the flow by hand.** Every action speaks plain names:
*Open a page → Take a screenshot*, *Open a page → Read the page → send to
Slack*. The Navigate action carries an Options section (window size,
wait-for-element, screenshot) so a full capture is ONE node.

**The "va sur ce site et extrais-moi ça" way — AI Agent.** The node is
`usableAsTool`: drop an **AI Agent** node (pick any chat model), add the
navette actions as its tools, and just tell it what you want:

> "Go to https://news.ycombinator.com and give me the top 5 titles with
> their points."

The agent opens pages, reads the markdown it gets back, clicks if it must,
and answers — no workflow wiring beyond the two nodes. Pair it with n8n's
built-in **Information Extractor** when you want the answer as strict JSON.

## What it unlocks

The n8n pain this node answers: *"this site has no API."* The pattern every
navette recipe uses, now inside workflows:

1. `navette creds set mysite` once (password lands in the OS keychain,
   bound to the origin) — then the **Login** node logs in *without the
   workflow ever seeing the password*; or log in by hand once and
   **Export State → Import State** the cookie jar between runs.
2. Navigate → Type → Click the real UI; Read/Screenshot to verify.
3. Same daemon, same sessions, ~1 ms actions — see `demo/wordpress` for the
   measured Playwright comparison.

## Install

```sh
# the daemon first (brew / cargo / docker — one binary)
navette serve --port 8765
```

In n8n: **Settings → Community nodes → Install** → `n8n-nodes-navette`
(published under this name; until then, local install below). Then create
the **navette daemon** credential (Base URL `http://127.0.0.1:8765`, Token
only if the daemon runs `--token`).

Local install for development:

```sh
cd integrations/n8n-nodes-navette && npm pack     # zero dependencies
docker cp navette-nodes-navette-0.1.0.tgz n8n:/tmp/
docker exec -it n8n npm install /tmp/navette-nodes-navette-0.1.0.tgz
docker restart n8n
```

If n8n runs in Docker, the daemon must be reachable from the container:
`http://host.docker.internal:8765` with `navette serve` on your host.

## Operations

| Operation | Body | Note |
|---|---|---|
| Navigate | url, withContent, format | redirects complete fine |
| Read | format | markdown for LLM steps |
| Screenshot | — | PNG as binary output (attach to email/Slack directly) |
| Click | selector, waitNavigation | pointer+mouse events — Radix menus open |
| Type | selector, value | React-safe native setter |
| Evaluate | js | escape hatch |
| Wait | selector, ms | polls until it exists |
| Login | site | `navette creds set`-registered credential; keychain-bound, origin-bound |
| Export / Import State | — | the cookie jar out and back in (storageState equivalent) |
| Sessions / Close | — | housekeeping |

Sessions are named contexts inside the daemon — one per site, cookies
persist between nodes and runs.

## The zero-dependency rule

The node wraps a loopback JSON API — there is no SDK to vendor and no
browser to download, which is the whole point. `package.json` ships with an
empty `dependencies` object on purpose; keep it that way.
