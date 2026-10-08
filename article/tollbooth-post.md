---
title: My agent met the real web. 403s, challenges, and the coming tollbooth.
published: false
description: 200 real URLs, one vanilla AI agent, zero challenge solving — 82.5% readable, 72 hosts blocking every AI crawler, zero x402 signals yet, and one named site already running a declared paywall with the gate up.
tags: ai, webdev, showdev, opensource
---

**Every agent demo you've seen runs on a friendly web. Mine went outside.**

We benchmark agents on static pages and local corpora because the real web is a hostile place to measure: walls move, challenges rotate, results depend on your IP's reputation that day. So almost nobody publishes the numbers. I wanted them, so I built the measurement: a vanilla agent — no challenge solving, no retries, no logins — opening 200 real URLs across five categories (docs, news, e-commerce, dev-tools, social) and trying to read each one.

The instrument is [navette](https://github.com/slabbdev/navette), my 659 KB browser-for-agents — but this post is about the web, not the tool. Any headless engine would produce the same shape of numbers; the dataset is linked at the end either way.

> **A measurement, not a verdict.** One exit IP (residential), one day, zero challenge solving, zero retries, zero logins. A site that walls itself off from this probe has not been judged — it has been measured once, from one vantage point.

## What a vanilla agent actually gets

| Category | URLs | readable | challenges | JS-gates | blocks all AI bots in robots.txt | llms.txt |
|---|---|---|---|---|---|---|
| docs | 40 | **39 (97%)** | 0 | 1 | 0 | 13 |
| news | 40 | 34 (85%) | 3 | 3 | **35** | 5 |
| e-commerce | 40 | 33 (82%) | 2 | 5 | 8 | 9 |
| dev-tools | 40 | 33 (82%) | 5 | 2 | 4 | 14 |
| social | 40 | 26 (65%) | 2 | **12** | 25 | 14 |

Aggregate: **165/200 — 82.5% — readable end-to-end by a vanilla agent.** Which sounds generous until you invert it: more than one page in six is closed, and the closed sixth is not randomly distributed. It concentrates exactly where agents add economic value — news, shopping, social.

The short version of the whole table: **documentation is free, and everything else negotiates.**

- **Docs: 39/40.** The single miss in the entire corpus is `go.dev/tour` — an interactive playground that builds itself in JS, not a documentation page. Every actual doc page — MDN, python.org, React, Rust, Kubernetes, AWS — served the agent exactly what it serves you. The web's public library is genuinely public.
- **News: 85% readable — but the wall is declared, not armed.** Three challenges, three JS-gates. The striking number sits in robots.txt: **35 of 40 news sites block every tracked AI crawler.** The robots file says no while the front door still opens. That gap is the interesting object: publishers are positioned to close it whenever the economics settle.
- **Social: 65%, and 12 of 40 are JS-gates.** The category that simply declines to render for strangers.

## The three walls, in order of arrival

**1. The declared wall (robots.txt).** The corpus tracks ten AI crawlers (GPTBot, ClaudeBot, CCBot, PerplexityBot, Bytespider…). **72 of 200 hosts block them all**; four more partially. The most-blocked bots: GPTBot (69 hosts), Google-Extended (67), Bytespider (67), ClaudeBot (66), anthropic-ai (66). A robots clause is free to declare and free to ignore — it's a social contract, not a fence. The measurement says the social contract is already signed everywhere the content is expensive to make.

**2. The enforced wall (challenges and JS-gates).** Turnstile, "Just a moment", DataDome, PerimeterX — **12 challenges** across the corpus, plus **23 empty/JS-gate pages** that render nothing without executing the gate. This is the fence. It's also the wall an agent *could* defeat and mostly *shouldn't* — defeating it is the compliance question, not the engineering question.

**3. The coming tollbooth (x402 and friends).** This is why I ran the bench twice. The x402 protocol family — HTTP 402 plus `X-Payment` headers, machine-payable per request — is moving from proposal to deployments. My probe watches for it: HTTP 402 status, payment headers, and the declared-paywall markup publishers already ship (`isAccessibleForFree:false` in schema.org — Google's paywall markup, readable by any agent that bothers to look).

**The result: zero.** Zero 402s, zero `X-Payment` headers, across the **164 probes that answered** — the honest qualifier is that 36 of 200 passive probes were themselves blocked by an edge (403), so absence is measured on what answered. The tollbooth is signed but not yet installed.

But I can name the shape of it, because one site in the corpus is already running all three layers at once:

> **The Intercept**: `isAccessibleForFree: false` in its markup (a declared paywall), **every tracked AI crawler blocked** in robots.txt (GPTBot, ClaudeBot, CCBot, Google-Extended, PerplexityBot, Amazonbot) — and its front page still fully readable, right now, by a vanilla agent.

That is a metered parking lot with the gate up. The prediction this dataset lets you test: the enforced wall (2) exists to protect the tollbooth (3) while it's being built. The markup is already telling you where the gate will come down.

And the walls move — that's the point of a dated, reproducible dataset. Between my October 6 and October 8 runs: one more news publisher flipped to block-all (34 → 35), and overall readability moved from 84.5% to 82.5%. Nobody notices one publisher flipping a robots line. The dataset does.

## llms.txt, honestly

55 of 200 sites ship an `llms.txt`. Adoption concentrates in docs and dev-tools — the categories with zero walls. The sites that most need a machine-negotiation channel (news, social) are the least interested in one: **five news sites total.** llms.txt is a courtesy lane on a road that's busy installing toll booths.

## The dataset

200 rows × 5 categories: outcome classification, wall type, robots.txt AI-clause stance per bot, llms.txt presence, x402/paywall signals, timings. **CC BY 4.0**, committed to the repo (`bench/tollbooth-results.{json,csv}` + `bench/DATASET.md` for the schema), reproducible with one command against any navette daemon:

```sh
NAVETTE_PORT=8766 python3 bench/tollbooth.py
```

**An open ask:** if you run agents against real sites daily — I'm looking at the crawler and extraction folks ([pulpie-mcp](https://github.com/pinkpixel-dev/pulpie-mcp) and neighbors) — send me your failure cases. The next run of this bench should include the URLs where *your* agents hit walls. That's how this becomes the reference dataset instead of my weekend run.

---

*The instrument, for the curious: [navette](https://github.com/slabbdev/navette) — a single Rust binary driving the WebView your OS already ships (WebKit/WebView2/WebKitGTK), 659 KB, MCP-native, MIT. The bench harness is `bench/tollbooth.py` in the same repo; the daemon it drives is the same one my agent uses to read this very web.*
