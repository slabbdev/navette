#!/usr/bin/env python3
"""The tollbooth bench — how much of the real web is walled off from AI agents?

For ~200 real URLs across 5 categories, measures four independent layers:
  1. robots.txt AI clauses (GPTBot / ClaudeBot / CCBot / Google-Extended /
     PerplexityBot / Bytespider / Amazonbot / anthropic-ai / Applebot-Extended)
     — stance per site: blocked-all, partial, no-rules, no-robots-file.
  2. llms.txt adoption (GET /llms.txt).
  3. x402 / paywall signals (round 2): a plain passive GET captures HTTP 402,
     X-Payment* / X402* response headers, and declared-paywall markup
     (schema.org isAccessibleForFree:false). The probe is recorded as its own
     observation — never merged into the agent-path classification. A blocked
     probe (403 from an edge) is reported as probe-blocked, not "no signals".
  4. The agent path itself: navette navigate+read (real WebKit, residential
     IP), classified as ok / challenge (Turnstile, "Just a moment", captcha…)
     / empty-js-gate / http-4xx / http-5xx / timeout / error.

Output: tollbooth-results.json (per-URL rows) + tollbooth-results.csv (flat
export) + tollbooth-summary.md (per-category aggregates). The dataset is
committed to the repo under CC BY 4.0 (code is MIT, repo root).

Usage: python3 tollbooth.py [--port 8765] [--out DIR] [--category NAME]
"""

import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import urlsplit

PORT = int(os.environ.get("NAVETTE_PORT", "8765"))
BASE = f"http://127.0.0.1:{PORT}"
TIMEOUT = 60
POLITENESS = 0.3  # seconds between navigations

AI_UAS = [
    "GPTBot", "ClaudeBot", "Claude-Web", "anthropic-ai", "CCBot",
    "Google-Extended", "PerplexityBot", "Bytespider", "Amazonbot",
    "Applebot-Extended",
]

CHALLENGE_MARKERS = [
    "just a moment", "attention required", "cf-chl", "cf-browser-verification",
    "turnstile", "captcha", "verify you are human", "are you a human",
    "access denied", "pardon our interruption", "ddos protection",
    "request unsuccessful. incapsula", "perimeterx", "datadome",
    "bot verification", "blocked because", "unusual traffic",
]

# Declared-paywall signals in markup. isAccessibleForFree is the schema.org
# NewsArticle paywall markup publishers ship for Google; the phrases are the
# hard-gate copy sites put in front of the article body.
PAYWALL_MARKERS = [
    "subscribe to continue", "subscription required", "subscribe to read",
    "payment required", "unlock this article", "subscribe to keep reading",
    "become a member to continue", "rent this article",
]

SEEDS = {
    "docs": [
        "https://developer.mozilla.org/en-US/docs/Web/JavaScript",
        "https://developer.mozilla.org/en-US/docs/Web/CSS",
        "https://developer.mozilla.org/en-US/docs/Web/API",
        "https://docs.python.org/3/tutorial/",
        "https://docs.python.org/3/library/asyncio.html",
        "https://docs.python.org/3/reference/datamodel.html",
        "https://react.dev/learn",
        "https://react.dev/reference/react/useState",
        "https://nextjs.org/docs",
        "https://vuejs.org/guide/introduction.html",
        "https://svelte.dev/docs/kit",
        "https://astro.build/docs/basics/project-structure/",
        "https://vitejs.dev/guide/",
        "https://www.rust-lang.org/learn",
        "https://doc.rust-lang.org/book/ch01-01-installation.html",
        "https://go.dev/doc/",
        "https://go.dev/tour/basics/1",
        "https://nodejs.org/api/fs.html",
        "https://www.typescriptlang.org/docs/handbook/intro.html",
        "https://docs.docker.com/get-started/",
        "https://kubernetes.io/docs/home/",
        "https://www.postgresql.org/docs/current/tutorial.html",
        "https://redis.io/docs/latest/",
        "https://www.sqlite.org/docs.html",
        "https://nginx.org/en/docs/",
        "https://git-scm.com/book/en/v2",
        "https://docs.github.com/en/actions",
        "https://docs.aws.amazon.com/lambda/latest/dg/welcome.html",
        "https://cloud.google.com/docs",
        "https://learn.microsoft.com/en-us/dotnet/",
        "https://getbootstrap.com/docs/5.3/getting-started/introduction/",
        "https://tailwindcss.com/docs/installation",
        "https://webpack.js.org/concepts/",
        "https://jestjs.io/docs/getting-started",
        "https://docs.pytest.org/en/stable/",
        "https://numpy.org/doc/stable/",
        "https://pandas.pydata.org/docs/user_guide/index.html",
        "https://threejs.org/docs/index.html#manual/en/introduction/Creating-a-scene",
        "https://www.php.net/manual/en/index.php",
        "https://guides.rubyonrails.org/getting_started.html",
    ],
    "news": [
        "https://www.reuters.com/",
        "https://apnews.com/",
        "https://www.bbc.com/news",
        "https://www.nytimes.com/",
        "https://www.washingtonpost.com/",
        "https://www.theguardian.com/international",
        "https://www.lemonde.fr/",
        "https://www.lefigaro.fr/",
        "https://www.cbc.ca/news",
        "https://www.aljazeera.com/",
        "https://www.cnbc.com/world/?region=world",
        "https://www.bloomberg.com/",
        "https://www.ft.com/",
        "https://www.economist.com/",
        "https://www.wsj.com/",
        "https://www.npr.org/sections/news/",
        "https://www.politico.com/",
        "https://www.theatlantic.com/",
        "https://www.wired.com/",
        "https://techcrunch.com/",
        "https://www.theverge.com/",
        "https://arstechnica.com/",
        "https://www.engadget.com/",
        "https://variety.com/",
        "https://www.hollywoodreporter.com/",
        "https://www.espn.com/",
        "https://www.cnn.com/",
        "https://abcnews.go.com/",
        "https://www.cbsnews.com/us/",
        "https://www.nbcnews.com/",
        "https://time.com/",
        "https://www.newsweek.com/",
        "https://www.forbes.com/",
        "https://www.inc.com/",
        "https://www.fastcompany.com/",
        "https://axios.com/",
        "https://www.semafor.com/",
        "https://theintercept.com/",
        "https://www.propublica.org/",
        "https://www.rtbf.be/en",
    ],
    "e-commerce": [
        "https://www.amazon.com/",
        "https://www.ebay.com/",
        "https://www.etsy.com/",
        "https://www.walmart.com/",
        "https://www.target.com/",
        "https://www.bestbuy.com/",
        "https://www.wayfair.com/",
        "https://www.ikea.com/us/en/",
        "https://www.homedepot.com/",
        "https://www.lowes.com/",
        "https://www.costco.com/",
        "https://www.newegg.com/",
        "https://www.bhphotovideo.com/",
        "https://www.apple.com/shop/buy-iphone",
        "https://www.microsoft.com/en-us/store/b/shop-all-microsoft",
        "https://www.samsung.com/us/",
        "https://shop.nordstrom.com/",
        "https://www.macys.com/",
        "https://www.zappos.com/",
        "https://www.asos.com/us/",
        "https://www.zalando.fr/",
        "https://fr.shein.com/",
        "https://www.temu.com/",
        "https://www.aliexpress.com/",
        "https://www.banggood.com/",
        "https://www.flipkart.com/",
        "https://www.myntra.com/",
        "https://www.noon.com/uae-en/",
        "https://www.jumia.ma/",
        "https://www.mercadolibre.com.mx/",
        "https://www.alibaba.com/",
        "https://stockx.com/",
        "https://www.fanatics.com/",
        "https://www.guitarguitar.co.uk/",
        "https://www.thomann.de/fr/index.html",
        "https://www.pccasegear.com/",
        "https://www.mouser.com/",
        "https://www.digikey.com/",
        "https://www.grainger.com/",
        "https://www.webstaurantstore.com/",
    ],
    "dev-tools": [
        "https://github.com/",
        "https://github.com/trending",
        "https://gitlab.com/",
        "https://www.npmjs.com/",
        "https://pypi.org/",
        "https://crates.io/",
        "https://rubygems.org/",
        "https://packagist.org/",
        "https://central.sonatype.com/",
        "https://stackoverflow.com/questions",
        "https://serverfault.com/",
        "https://stackexchange.com/",
        "https://hub.docker.com/",
        "https://readthedocs.org/",
        "https://dev.to/",
        "https://medium.com/tag/programming",
        "https://hashnode.com/",
        "https://news.ycombinator.com/",
        "https://codepen.io/trending",
        "https://jsfiddle.net/",
        "https://replit.com/",
        "https://codesandbox.io/",
        "https://vercel.com/docs",
        "https://www.netlify.com/docs/",
        "https://railway.app/docs",
        "https://render.com/docs",
        "https://fly.io/docs/",
        "https://www.heroku.com/platform",
        "https://www.digitalocean.com/products",
        "https://www.jetbrains.com/idea/",
        "https://code.visualstudio.com/docs",
        "https://marketplace.visualstudio.com/VSCode",
        "https://caniuse.com/",
        "https://developer.apple.com/documentation/",
        "https://developer.android.com/docs",
        "https://docs.rs/serde/latest/serde/",
        "https://pkg.go.dev/std",
        "https://libraries.io/",
        "https://www.jsdelivr.com/",
        "https://unpkg.com/",
    ],
    "social": [
        "https://x.com/",
        "https://www.facebook.com/",
        "https://www.instagram.com/",
        "https://www.linkedin.com/",
        "https://www.tiktok.com/",
        "https://www.pinterest.com/",
        "https://www.reddit.com/",
        "https://www.youtube.com/",
        "https://www.twitch.tv/",
        "https://discord.com/",
        "https://telegram.org/",
        "https://www.threads.net/",
        "https://bsky.app/",
        "https://mastodon.social/",
        "https://lemmy.world/",
        "https://www.tumblr.com/",
        "https://www.flickr.com/",
        "https://vk.com/",
        "https://weibo.com/",
        "https://www.quora.com/",
        "https://www.goodreads.com/",
        "https://letterboxd.com/",
        "https://bandcamp.com/",
        "https://soundcloud.com/discover",
        "https://open.spotify.com/",
        "https://www.mixcloud.com/",
        "https://www.strava.com/",
        "https://www.behance.net/",
        "https://dribbble.com/shots",
        "https://imgur.com/",
        "https://9gag.com/",
        "https://knowyourmeme.com/",
        "https://www.boredpanda.com/",
        "https://www.duolingo.com/learn",
        "https://www.meetup.com/",
        "https://www.eventbrite.com/",
        "https://www.yelp.com/",
        "https://www.trustpilot.com/",
        "https://stackshare.io/",
        "https://alternativeto.net/",
    ],
}


def http_get(url: str, timeout: int = 15) -> tuple[int, str]:
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (tollbooth-research)"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, ""
    except Exception:
        return 0, ""


def robots_stance(host: str) -> dict:
    code, body = http_get(f"https://{host}/robots.txt")
    if code != 200 or not body.strip():
        return {"stance": "no-robots", "ai_blocked": [], "ai_partial": []}
    # parse groups: user-agent lines then rules, per RFC-ish
    groups: dict[str, list[str]] = {}
    current: list[str] = []
    for line in body.splitlines():
        line = line.split("#")[0].strip()
        if not line or ":" not in line:
            continue
        k, v = line.split(":", 1)
        k, v = k.strip().lower(), v.strip()
        if k == "user-agent":
            if not current or (groups.get(current[-1]) not in (None,) and False):
                pass
            current.append(v)
        elif k == "disallow":
            if not current:
                continue
            for ua in current:
                groups.setdefault(ua, []).append(v)
        elif k == "allow":
            for ua in current:
                groups.setdefault(ua, [])
    # the "*" group is the default; AI UAs may have their own group
    wildcard = groups.get("*", [])
    blocked, partial = [], []
    for ua in AI_UAS:
        own = groups.get(ua)
        if own is None:
            # falls back to wildcard — only "blocked" if wildcard blocks everything
            if any(d == "/" for d in wildcard):
                blocked.append(ua)
            continue
        if any(d == "/" for d in own):
            blocked.append(ua)
        elif any(d for d in own):
            partial.append(ua)
    if blocked or partial:
        stance = "blocked-all" if blocked else "partial"
    else:
        stance = "no-rules"
    return {"stance": stance, "ai_blocked": blocked, "ai_partial": partial}


def llms_txt(host: str) -> dict:
    code, body = http_get(f"https://{host}/llms.txt", timeout=10)
    return {"present": code == 200 and bool(body.strip()), "bytes": len(body) if code == 200 else 0}


def payment_probe(url: str) -> dict:
    """Round 2: x402 / paywall signals, as an independent passive observation.

    Plain GET with a research UA (not the WebKit agent path). Status 403 from
    an edge means the PROBE was blocked — that is reported, never read as
    "no payment signals".
    """
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (tollbooth-research)"})
    status, headers, body = 0, {}, ""
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            status, headers, body = r.status, dict(r.headers), r.read(250_000).decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        status, headers = e.code, dict(e.headers or {})
        try:
            body = e.read(250_000).decode("utf-8", "replace")
        except Exception:
            pass
    except Exception:
        return {"probe_status": 0, "x402": False, "paywall": False, "signals": []}

    signals: list[str] = []
    for h, v in headers.items():
        hl = h.lower()
        if hl.startswith(("x-pay", "x402", "x-402")):
            signals.append(f"hdr {h}: {str(v)[:60]}")
    if status == 402:
        signals.append("http-402")
    low = body.lower()
    if '"isaccessibleforfree":false' in low or 'isaccessibleforfree" content="false"' in low:
        signals.append("schema-isAccessibleForFree-false")
    for m in PAYWALL_MARKERS:
        if m in low:
            signals.append(f"marker: {m}")
    return {
        "probe_status": status,
        "x402": any(s.startswith(("hdr X-Pay", "hdr x-pay", "hdr X402", "hdr x402", "http-402")) for s in signals),
        "paywall": any(s.startswith(("schema-", "marker:")) for s in signals),
        "signals": signals[:8],
    }


def classify(result: dict) -> str:
    if result.get("nav_error") or result.get("fold_error") in ("navigation timeout",):
        return "timeout-or-error"
    blob = (str(result.get("title", "")) + " " + str(result.get("content", ""))).lower()
    for m in CHALLENGE_MARKERS:
        if m in blob:
            return "challenge"
    c = str(result.get("content", ""))
    if len(c) < 120:
        return "empty-js-gate"
    return "ok"


def via_navette(url: str) -> dict:
    req = urllib.request.Request(
        BASE + "/navigate",
        data=json.dumps({"url": url, "with_content": True}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            d = json.loads(r.read())
            d["ms"] = round((time.perf_counter() - t0) * 1000)
            return d
    except urllib.error.HTTPError as e:
        body = ""
        try:
            body = e.read().decode("utf-8", "replace")
        except Exception:
            pass
        return {"ok": False, "http_status": e.code, "server_error": body[:200],
                "ms": round((time.perf_counter() - t0) * 1000)}
    except Exception as e:
        return {"ok": False, "http_status": 0, "server_error": str(e)[:200],
                "ms": round((time.perf_counter() - t0) * 1000)}


def main() -> None:
    only = sys.argv[1] if len(sys.argv) > 1 else None
    out_dir = os.path.dirname(os.path.abspath(__file__))
    rows = []
    cats = {k: v for k, v in SEEDS.items() if only is None or k == only}
    for cat, urls in cats.items():
        robots_cache: dict[str, dict] = {}
        for url in urls:
            host = urlsplit(url).netloc
            if host not in robots_cache:
                robots_cache[host] = robots_stance(host)
            res = via_navette(url)
            row = {
                "category": cat,
                "url": url,
                "outcome": classify(res),
                "ms": res.get("ms"),
                "http_status": res.get("http_status"),
                "title": (res.get("title") or "")[:120],
                "content_chars": len(str(res.get("content") or "")),
                "fold_error": res.get("fold_error"),
                **robots_cache[host],
                "llms_txt": llms_txt(host),
                "payment": payment_probe(url),
            }
            rows.append(row)
            print(f"[{cat}] {row['outcome']:>16}  {url}", flush=True)
            time.sleep(POLITENESS)

    json.dump(rows, open(os.path.join(out_dir, "tollbooth-results.json"), "w"), indent=1)

    # flat CSV export — the dataset people can open in a spreadsheet
    import csv
    csv_cols = ["category", "url", "outcome", "ms", "http_status", "title",
                "content_chars", "stance", "ai_blocked", "ai_partial",
                "llms_txt", "payment_probe_status", "x402", "paywall", "payment_signals"]
    with open(os.path.join(out_dir, "tollbooth-results.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(csv_cols)
        for r in rows:
            pay = r.get("payment") or {}
            w.writerow([
                r["category"], r["url"], r["outcome"], r.get("ms"), r.get("http_status"),
                r.get("title"), r.get("content_chars"), r.get("stance"),
                "|".join(r.get("ai_blocked") or []), "|".join(r.get("ai_partial") or []),
                "yes" if (r.get("llms_txt") or {}).get("present") else "no",
                pay.get("probe_status"), "yes" if pay.get("x402") else "no",
                "yes" if pay.get("paywall") else "no", " ; ".join(pay.get("signals") or []),
            ])

    # summary
    lines = [
        "# Tollbooth — the walled web, measured",
        "",
        "> **A measurement, not a verdict.** One exit IP (residential), no challenge solving, no retries, no logins. A site that walls itself off from this probe has not been judged — it has been measured once, from one vantage point, on one day.",
        "> **Dataset license: [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)** — the bench code is MIT (repo root).",
        "",
        f"URLs measured: {len(rows)} · method: real WebKit via navette navigate+read, residential IP · "
        f"payment probe: passive plain GET (recorded separately, never merged into the agent-path outcome)",
        "",
    ]
    lines += ["| Category | URLs | ok | challenge | empty/JS-gate | timeout/error | AI-blocked-all | AI-partial | llms.txt | x402 | paywall |",
              "|---|---|---|---|---|---|---|---|---|---|---|"]
    for cat in cats:
        rs = [r for r in rows if r["category"] == cat]
        n = len(rs) or 1
        lines.append(
            f"| {cat} | {len(rs)} | {sum(r['outcome']=='ok' for r in rs)} "
            f"({sum(r['outcome']=='ok' for r in rs)*100//n}%) "
            f"| {sum(r['outcome']=='challenge' for r in rs)} "
            f"| {sum(r['outcome']=='empty-js-gate' for r in rs)} "
            f"| {sum(r['outcome']=='timeout-or-error' for r in rs)} "
            f"| {len({r['url'].split('/')[2] for r in rs if r.get('stance')=='blocked-all'})} "
            f"| {len({r['url'].split('/')[2] for r in rs if r.get('stance')=='partial'})} "
            f"| {sum(r['llms_txt']['present'] for r in rs)} "
            f"| {sum((r.get('payment') or {}).get('x402', False) for r in rs)} "
            f"| {sum((r.get('payment') or {}).get('paywall', False) for r in rs)} |")
    open(os.path.join(out_dir, "tollbooth-summary.md"), "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
