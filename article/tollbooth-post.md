---
title: My agent met the real web. 403s, challenges, and the coming tollbooth.
published: false
description: 200 real URLs, one vanilla AI agent, zero challenge solving — a measurement of how much of the web is walled off, and the first look at x402 pay-per-request signals.
tags: ai, webdev, showdev, opensource
---

**Every agent demo you've seen runs on a friendly web. Mine went outside.**

We benchmark agents on static pages and local corpora because the real web is a hostile place to measure: walls move, challenges rotate, and results depend on your IP's reputation that day. So almost nobody publishes the numbers. I wanted them, so I built the measurement: a vanilla agent — no challenge solving, no retries, no logins — opening 200 real URLs across five categories (docs, news, e-commerce, dev-tools, social) and trying to read each one.

The instrument is [navette](https://github.com/slabbdev/navette), my 659 KB browser-for-agents — but this post is about the web, not the tool. Any headless engine would produce the same shape of numbers; the dataset is linked at the end either way.

> **A measurement, not a verdict.** One exit IP (residential), one day, zero challenge solving, zero retries, zero logins. A site that walls itself off from this probe has not been judged — it has been measured once.

## What a vanilla agent actually gets

<!-- NRS: aggregate table from tollbooth-summary.md — paste fresh numbers -->

The short version: **documentation is free, and everything else negotiates.**

- **Docs: 100% readable. Zero walls.** Every documentation site in the corpus — MDN, python.org, React, Rust, Kubernetes, AWS — served the agent exactly what it serves you. The web's public library is genuinely public.
- **News: 85% readable — but the wall is declared, not armed.** Four challenges, two JS-gates. The striking number is elsewhere: **34 of 40 news sites block every tracked AI crawler in robots.txt.** The robots file says no while the front door still opens. That gap is the interesting object: publishers are positioned to close it whenever the economics settle.
- **E-commerce: 80%.** Three challenges, five JS-gates — bot defense where the money is.
- **Dev-tools: 87%.** Five challenges, including places you'd not expect.
- **Social: 70%, and 12 of 40 are JS-gates.** The category that simply declines to render for strangers.

Aggregate: **169/200 — 84.5% — readable end-to-end by a vanilla agent.** Which sounds generous until you invert it: one page in seven is closed, and the closed seventh is not randomly distributed. It concentrates exactly where agents add economic value — news, shopping, social.

## The three walls, in order of arrival

**1. The declared wall (robots.txt).** The corpus tracks ten AI crawlers (GPTBot, ClaudeBot, CCBot, PerplexityBot, Bytespider…). <!-- NRS: fresh totals --> sites block them all, <!-- NRS --> partially. A robots clause is free to declare and free to ignore — it's a social contract, not a fence. The measurement says the social contract is already signed everywhere the content is expensive to make.

**2. The enforced wall (challenges and JS-gates).** Turnstile, "Just a moment", DataDome, PerimeterX — <!-- NRS: total challenges --> challenges across the corpus, plus <!-- NRS --> empty/JS-gate pages that render nothing without executing the gate. This is the fence. It's also the wall an agent *could* defeat and mostly *shouldn't* — defeating it is the compliance question, not the engineering question.

**3. The coming tollbooth (x402 and friends).** This is why I re-ran the bench. The x402 protocol family — HTTP 402 plus `X-Payment` headers, machine-payable per request — is moving from proposal to deployments. My round-2 probe watches for it: HTTP 402 status, payment headers, and the declared-paywall markup publishers already ship (`isAccessibleForFree:false` in schema.org — Google's paywall markup, readable by any agent that bothers to look).

<!-- NRS: x402/paywall findings from the fresh run — the honest number, even if it's zero. "Zero 402s in the wild today, N declared paywalls via isAccessibleForFree, and here is what that means: the tollbooth is signed but not yet installed." -->

The prediction this dataset lets you test: the enforced wall (2) exists to protect the tollbooth (3) while it's being built. News sites declaring `isAccessibleForFree:false` **and** blocking all AI crawlers **and** still serving a vanilla agent the front page are running a metered parking lot with the gate up. The gate will come down; the markup is already telling you where.

## llms.txt, honestly

54 of 200 sites ship an `llms.txt`. Adoption concentrates in docs and dev-tools — the categories with zero walls. The sites that most need a machine-negotiation channel (news, social) are the least interested in one. llms.txt is a courtesy lane on a road that's busy installing toll booths.

## The dataset

200 rows × 5 categories: outcome classification, wall type, robots.txt AI-clause stance per bot, llms.txt presence, payment/x402 signals, timings. **CC BY 4.0**, in the repo, reproducible with one command:

<!-- NRS: link + one-command repro + round-2 numbers -->

**An open ask:** if you run agents against real sites daily — I'm looking at the crawler and extraction folks ([pulpie-mcp](https://github.com/pinkpixel-dev/pulpie-mcp) and neighbors) — send me your failure cases. The next run of this bench should include the URLs where *your* agents hit walls. That's how this becomes the reference dataset instead of my weekend run.

---

*The instrument, for the curious: [navette](https://github.com/slabbdev/navette) — a single Rust binary driving the WebView your OS already ships (WebKit/WebView2/WebKitGTK), 659 KB, MCP-native, MIT. The bench harness is `bench/tollbooth.py` in the same repo; the daemon it drives is the same one my agent uses to read this very web.*
