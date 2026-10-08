# The tollbooth dataset — schema

**Dataset license: [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).** The bench code (`tollbooth.py`) is MIT, like the rest of the repo. Cite the run date and the method; the numbers are a measurement from one vantage point, not a verdict about any site.

Files: `tollbooth-results.json` (full rows), `tollbooth-results.csv` (flat export), `tollbooth-summary.md` (per-category aggregates).

## Method

- **Agent path** — real WebKit (system WebView) via `navette navigate+read`, from one residential exit IP, no challenge solving, no retries, no logins. Classified `ok` / `challenge` / `empty-js-gate` / `timeout-or-error`.
- **robots.txt stance** — parsed per host: which of the 10 tracked AI crawlers (GPTBot, ClaudeBot, Claude-Web, anthropic-ai, CCBot, Google-Extended, PerplexityBot, Bytespider, Amazonbot, Applebot-Extended) are blocked-all / partial, per that bot's own group with wildcard fallback.
- **llms.txt** — `GET /llms.txt`, present if 200 with a non-empty body.
- **Payment probe (round 2)** — a passive plain GET with a research UA, recorded **separately** from the agent path: HTTP 402, `X-Payment*` / `X402*` response headers (the x402 protocol family), schema.org `isAccessibleForFree:false` paywall markup, and hard paywall phrases. A probe blocked by an edge (403) is reported as such — it is not evidence of "no signals".

## Row schema (JSON)

| field | meaning |
|---|---|
| `category` | docs / news / e-commerce / dev-tools / social |
| `url` | the measured URL |
| `outcome` | agent-path classification (see Method) |
| `ms` | navigate+read wall time via the resident daemon |
| `http_status` | navette HTTP status if the call itself failed; else null |
| `title` / `content_chars` | page title; extracted content length |
| `fold_error` | navette fold error, if any |
| `stance` | robots.txt AI stance: blocked-all / partial / no-rules / no-robots |
| `ai_blocked` / `ai_partial` | lists of blocked / partially-restricted AI crawlers |
| `llms_txt` | `{present, bytes}` |
| `payment` | `{probe_status, x402, paywall, signals}` — the passive payment probe |

The CSV flattens the same rows; `ai_blocked`/`ai_partial` are `|`-joined, `payment.signals` are `;`-joined.

## Re-run

```sh
navette serve --port 8766 &            # any navette daemon
NAVETTE_PORT=8766 python3 bench/tollbooth.py          # full run (~200 URLs)
NAVETTE_PORT=8766 python3 bench/tollbooth.py docs     # one category
```

A full run rewrites `tollbooth-results.json`, `.csv` and `tollbooth-summary.md`. Round 1 (no payment probe) is preserved in git history (commit 29858c0).
