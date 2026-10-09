#!/usr/bin/env python3
"""Price stats from leboncoin.fr through navette — read-only, no API.

leboncoin has no public read API and sits behind Datadome, yet the page a
human sees in Safari renders the same in navette: the system WebView with a
residential exit IP IS the human path. This script drives one search page
through navette's HTTP primitives — navigate, wait, evaluate — and folds the
listings into price stats. Read-only: no login, no captcha handling, no
retries. If the site serves a challenge instead of listings, it reports
`challenge` and stops, honestly.

Usage:

  stats QUERY [--pages N] [--category ID] [--param k=v]... [--json OUT] [--shot PNG]

  stats "macbook pro"                          # page 1 of the text search
  stats "smartphone samsung" --category 15     # category = leboncoin's numeric id
  stats "iphone 15" --pages 2 --param locations=Paris

Environment: NAVETTE (default http://127.0.0.1:8765), session name fixed to
"leboncoin". Politeness is a feature: one request per page, 2.5 s between
pages, never more than 3 pages, no retry loops. Personal-use scale only —
this is a recipe for "what does X cost today", not bulk scraping; leboncoin's
ToS restrict automated access, so keep it human-sized and infrequent.

Facts the extraction rests on (DOM verified live 2026-10-09):
  - search page   /recherche?text=…&page=N → cards in [data-qa-id="aditem_container"]
  - consent       Didomi banner dims the page; [data-qa-id="didomi_cta_continue_without_consent"]
                  ("Continuer sans accepter") dismisses it without granting anything
  - interstitial  a Gimii donation <dialog id="gimii-root"> (third-party widget)
                  pops seconds after load and dims the page; no close control is
                  exposed, so the script closes the native dialog element — its
                  own Close path — and drops the leftover overlay div. It is a
                  UI layer, not an access gate: no captcha is touched.
  - total         H2 "Résultats de recherche : <n> annonces" → market-size stat
  - title         article[aria-label] / p.text-body-1-highlight (visible one)
  - price         p.text-callout, French format "2 990 €" → int
  - meta          p.text-caption: [category, "City INSEE"]  (date not exposed
                  on every slot — kept when present)
  - pro seller    sr-only paragraph mentions "Vendeur professionnel"
  - link          a[href^="/ad/"] relative, absolute-joined
  - featured ads  ("À la une") use the same container — deduped by ad id
"""

import argparse
import json
import os
import re
import statistics
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

NAVETTE = os.environ.get("NAVETTE", "http://127.0.0.1:8765")
SESSION = "leboncoin"
BASE = "https://www.leboncoin.fr"
MAX_PAGES = 3          # politeness ceiling, not a technical limit
PAGE_SLEEP = 2.5       # seconds between page navigations

# One /evaluate call per page — everything computed browser-side, nothing
# round-tripped. Names match the DOM facts above; class names are Tailwind
# tokens leboncoin uses for type roles (fragile-ish, hence the € regex
# fallback on the price line).
EXTRACT_JS = r"""
var total=null;
var h=[].slice.call(document.querySelectorAll('h2')).find(function(e){
  return /annonces/i.test(e.textContent)});
if(h){var m=h.textContent.match(/(\d[\d\u202f\s]*)\s*annonces/i);
  if(m) total=parseInt(m[1].replace(/[^\d]/g,''),10)}
var containers=[].slice.call(document.querySelectorAll('[data-qa-id="aditem_container"]'));
function txt(e){return e?e.textContent.replace(/\s+/g,' ').trim():null}
var seen={};
var ads=containers.map(function(c){
  var link=c.querySelector('a[href^="/ad/"]');
  var href=link?link.getAttribute('href'):null;
  if(!href) return null;
  var id=(href.match(/\/(\d+)(?:\/|$)/)||[])[1]||href;
  if(seen[id]) return null; seen[id]=1;             // featured dupes
  var title=txt(c.querySelector('p.text-body-1-highlight')) ||
            (c.closest('article')&&c.closest('article').getAttribute('aria-label'));
  var priceEl=c.querySelector('p.text-callout');
  var price=priceEl?parseInt(priceEl.textContent.replace(/[^\d]/g,''),10):null;
  var caps=[].slice.call(c.querySelectorAll('p')).filter(function(p){
    return (p.className||'').indexOf('text-caption')>=0;
  }).map(txt).filter(Boolean);
  var sr=[].slice.call(c.querySelectorAll('p')).map(txt).join(' ');
  return {id:id,title:title,price:price,category:caps[0]||null,location:caps[1]||null,
          pro:/professionnel/i.test(sr),url:href?('https://www.leboncoin.fr'+href):null};
}).filter(Boolean);
JSON.stringify({total:total,n:containers.length,ads:ads});
"""


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def api(path, body=None, raw=False, timeout=60):
    url = f"{NAVETTE}{path}"
    data = json.dumps(body or {}).encode()
    req = urllib.request.Request(url, data=data, method="POST",
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            payload = r.read()
    except urllib.error.HTTPError as e:
        die(f"navette {path} -> HTTP {e.code}: {e.read().decode(errors='replace')[:200]}")
    except urllib.error.URLError as e:
        die(f"cannot reach navette at {NAVETTE} ({e.reason}) — is `navette serve` running?")
    if raw:
        return payload
    try:
        return json.loads(payload)
    except json.JSONDecodeError:
        die(f"navette {path} returned non-JSON")


def search_url(query, page, category=None, extra=None):
    q = {"text": query}
    if category:
        q["category"] = category
    if extra:
        q.update(extra)
    if page > 1:
        q["page"] = page
    return f"{BASE}/recherche?{urllib.parse.urlencode(q)}"


def looks_like_challenge(page):
    t = (page.get("title") or "").lower()
    markers = ("datadome", "captcha", "pardon", "vérification", "blocked")
    return any(m in t for m in markers) or "geo.captcha-delivery.com" in (page.get("url") or "")


def settle_page(page_no):
    """Clear the two UI layers that dim the page. Both are presence-tested —
    the banner and the interstitial appear on their own schedule, so every
    step is best-effort."""
    # Didomi consent banner → "Continuer sans accepter" grants nothing.
    got_banner = api("/evaluate", {"session": SESSION, "js":
        "JSON.stringify(!!document.querySelector('[data-qa-id=\"didomi_cta_continue_without_consent\"]'))"})
    if json.loads(got_banner.get("result", "false")):
        api("/click", {"selector": '[data-qa-id="didomi_cta_continue_without_consent"]',
                       "session": SESSION})
        time.sleep(0.8)
    # Gimii donation interstitial → close the native <dialog>; not an access gate.
    api("/evaluate", {"session": SESSION, "js":
        "var d=document.getElementById('gimii-root'); if(d&&d.open)d.close();"
        "var o=document.querySelector('[class*=gimii_overlay]'); if(o)o.remove();"})
    time.sleep(0.3)


def harvest_page(query, page, category, extra):
    url = search_url(query, page, category, extra)
    nav = api("/navigate", {"url": url, "session": SESSION})
    if looks_like_challenge(nav):
        return {"outcome": "challenge", "url": url, "title": nav.get("title")}
    api("/wait", {"selector": '[data-qa-id="aditem_container"]', "ms": 15000,
                  "session": SESSION})
    time.sleep(1.5)  # React hydration: text roles fill after the containers mount
    settle_page(page)
    ev = api("/evaluate", {"js": EXTRACT_JS, "session": SESSION})
    try:
        payload = json.loads(ev["result"])
    except (KeyError, json.JSONDecodeError):
        return {"outcome": "unreadable", "url": url}
    return {"outcome": "ok", "url": url, "title": nav.get("title"), **payload}


def stats_block(prices):
    if not prices:
        return {}
    s = sorted(prices)
    return {"count": len(s), "min": s[0], "median": int(statistics.median(s)),
            "mean": int(statistics.fmean(s)), "p90": s[int(0.9 * (len(s) - 1))],
            "max": s[-1]}


def summarize(pages, query):
    ads, challenges = [], 0
    for p in pages:
        if p["outcome"] == "ok":
            ads += p["ads"]
        elif p["outcome"] == "challenge":
            challenges += 1
    prices = [a["price"] for a in ads if a["price"]]
    locs = {}
    for a in ads:
        if a["location"]:
            city = re.sub(r"\s+\d{5}.*$", "", a["location"]).strip()
            locs[city] = locs.get(city, 0) + 1
    cats = {}
    for a in ads:
        if a["category"]:
            cats[a["category"]] = cats.get(a["category"], 0) + 1
    return {
        "query": query, "pages": [p["outcome"] for p in pages],
        "ads": len(ads), "challenges": challenges,
        "total_announces": next((p.get("total") for p in pages if p.get("total")), None),
        "price_eur": stats_block(prices),
        "pro_share": round(sum(a["pro"] for a in ads) / len(ads), 2) if ads else None,
        "top_locations": sorted(locs.items(), key=lambda kv: -kv[1])[:5],
        "categories": sorted(cats.items(), key=lambda kv: -kv[1])[:5],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    st = sub.add_parser("stats", help="navigate a leboncoin search and fold listings into stats")
    st.add_argument("query")
    st.add_argument("--pages", type=int, default=1, choices=range(1, MAX_PAGES + 1))
    st.add_argument("--category", default=None, help="leboncoin numeric category id")
    st.add_argument("--param", action="append", default=[], metavar="K=V",
                    help="extra search param, repeatable (e.g. locations=Paris)")
    st.add_argument("--json", dest="out", default=None, help="write full result JSON here")
    st.add_argument("--shot", dest="shot", default=None, help="write page-1 screenshot here")
    args = ap.parse_args()

    extra = dict(kv.split("=", 1) for kv in args.param)
    pages = []
    for page in range(1, args.pages + 1):
        pages.append(harvest_page(args.query, page, args.category, extra))
        if page < args.pages:
            time.sleep(PAGE_SLEEP)

    if args.shot:
        settle_page(1)  # the Gimii dialog can pop between harvest and capture
        api("/scroll", {"selector": '[data-qa-id="aditem_container"]', "session": SESSION})
        time.sleep(0.6)
        Path(args.shot).write_bytes(api("/screenshot", {"session": SESSION}, raw=True))
        print(f"screenshot -> {args.shot}")

    summary = summarize(pages, args.query)
    if args.out:
        Path(args.out).write_text(json.dumps(
            {"summary": summary, "pages": pages}, ensure_ascii=False, indent=2) + "\n")
        print(f"json -> {args.out}")

    if summary["challenges"]:
        print(f"outcome: {summary['challenges']}/{len(pages)} page(s) behind a challenge — "
              f"read what a human reads, stop, try later; no captcha handling by design.")
    u = "€"
    print(f"\nleboncoin — {args.query!r}  ({summary['ads']} ads scraped, "
          f"pages: {', '.join(summary['pages'])})")
    if summary.get("total_announces"):
        print(f"  market: {summary['total_announces']:,} annonces au total".replace(",", " "))
    if summary["price_eur"]:
        p = summary["price_eur"]
        print(f"  price  min {p['min']}{u} · median {p['median']}{u} · "
              f"mean {p['mean']}{u} · p90 {p['p90']}{u} · max {p['max']}{u}")
    if summary["pro_share"] is not None:
        print(f"  pro sellers: {summary['pro_share']:.0%}")
    for label, key in (("top locations", "top_locations"), ("categories", "categories")):
        if summary[key]:
            row = ", ".join(f"{k} ({v})" for k, v in summary[key])
            print(f"  {label}: {row}")


if __name__ == "__main__":
    main()
