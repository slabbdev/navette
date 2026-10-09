#!/usr/bin/env python3
"""WordPress smoke E2E with navette — no browser downloaded, ever.

The counter-demo to "Playwright for WordPress": the same journeys (install,
login, publish in the block editor, verify the frontend, leave a comment)
driven over navette's HTTP primitives against a disposable docker WordPress.

    docker compose up -d                      # http://127.0.0.1:8090
    NAVETTE=http://127.0.0.1:8765 python3 wp-smoke.py

Exit code 0 = every journey passed. Artifacts (screenshots) land in
./artifacts/. Requires a navette build with the macOS /wait fix and
pointer-event /click (2026-10-08 or later).

What this suite is: SMOKE tier — "does the journey still work", with plain
Python asserts over navette reads. Not an assertion framework, no trace
viewer; the trade is spelled out in README.md.
"""

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

NAVETTE = os.environ.get("NAVETTE", "http://127.0.0.1:8765")
HERE = Path(__file__).resolve().parent
ART = HERE / "artifacts"
ART.mkdir(exist_ok=True)

ADMIN_USER = "smokeadmin"
ADMIN_PASS = "navette-smoke-2026!"   # container-only credential, safe to commit
SITE_TITLE = "navette Smoke"

RESULTS = []
T0 = time.time()


def api(path, body=None, timeout=90):
    data = json.dumps(body or {}).encode()
    req = urllib.request.Request(f"{NAVETTE}{path}", data=data, method="POST",
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")[:200]
        fail(path, f"HTTP {e.code}: {detail}")
    except urllib.error.URLError as e:
        fail(path, f"cannot reach navette at {NAVETTE} ({e.reason})")


def ev(session, js):
    """evaluate, always returning parsed JSON (stringify in the page: the
    macOS bridge serializes objects as NSDictionary descriptions)."""
    raw = api("/evaluate", {"js": f"(function(){{try{{return JSON.stringify({js})}}catch(e){{return JSON.stringify({{err:String(e)}})}}}})()", "session": session}).get("result")
    if raw in (None, ""):
        return {}
    try:
        return json.loads(raw)
    except (TypeError, ValueError):
        return {"raw": raw}


def check(step, ok, detail=""):
    RESULTS.append((step, ok, detail))
    mark = "✓" if ok else "✗"
    print(f"  {mark} {step:<28} {detail}", flush=True)


def fail(step, msg):
    check(step, False, msg)
    report()
    sys.exit(1)


def screenshot(session, name):
    req = urllib.request.Request(f"{NAVETTE}/screenshot",
                                 data=json.dumps({"session": session}).encode(),
                                 method="POST",
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        png = r.read()
    (ART / name).write_bytes(png)
    return len(png)


def report():
    total = time.time() - T0
    passed = sum(1 for _, ok, _ in RESULTS if ok)
    print(f"\n{'='*62}\n  {passed}/{len(RESULTS)} journeys passed in {total:.1f} s — navette {NAVETTE}")
    for step, ok, detail in RESULTS:
        print(f"  {'✓' if ok else '✗'} {step:<30} {detail[:70]}")
    print("=" * 62)


# ---------- Gutenberg lives in a same-origin blob iframe (WP 6.3+).
# There is no navette iframe API — none is needed: /evaluate hops through
# contentDocument like any same-origin page JS would.

CANVAS = ("(document.querySelector('iframe[name=editor-canvas]')||document.querySelector('iframe'))"
          "?.contentDocument")


def journey_install(url):
    """First boot only: the famous 5-minute install, in 5 seconds."""
    probe = ev("wpadmin", "location.pathname")
    r = api("/navigate", {"url": f"{url}/wp-admin/", "session": "wpadmin"})
    state = ev("wpadmin", "location.pathname")
    if "/wp-login.php" in str(state):
        check("install", True, "already installed — skipped")
        return
    # language step (first boot): Continue reloads the page with the real form
    for _ in range(10):
        has_lang = api("/evaluate", {"session": "wpadmin",
                                     "js": "String(!!document.querySelector('#language-continue'))"}).get("result")
        if has_lang == "true":
            api("/click", {"selector": "input#language-continue", "session": "wpadmin"})
        time.sleep(1)
        has_form = api("/evaluate", {"session": "wpadmin",
                                     "js": "String(!!document.querySelector('input[name=weblog_title]'))"}).get("result")
        if has_form == "true":
            break
    else:
        fail("install", "install form never appeared")
    for sel, val in [("input[name=weblog_title]", SITE_TITLE),
                     ("input[name=user_name]", ADMIN_USER),
                     ("input[name=admin_password]", ADMIN_PASS),
                     ("input[name=admin_password2]", ADMIN_PASS),
                     ("input[name=admin_email]", "smoke@local.test")]:
        res = api("/type", {"selector": sel, "value": val, "session": "wpadmin"})
        if "OK" not in str(res.get("result")):
            fail("install", f"could not fill {sel}: {res}")
    api("/click", {"selector": "input[name=Submit]", "session": "wpadmin", "wait_navigation": True})
    time.sleep(1)
    done = ev("wpadmin", "(document.querySelector('h1')||{textContent:''}).textContent")
    ok = "Success" in str(done)
    check("install (wizard)", ok, f"h1={done!r}" if not ok else "5-minute install in 5 s")
    if not ok:
        fail("install", "wizard did not report Success")


def journey_login(url):
    api("/navigate", {"url": f"{url}/wp-login.php", "session": "wpadmin"})
    api("/type", {"selector": "input[name=log]", "value": ADMIN_USER, "session": "wpadmin"})
    api("/type", {"selector": "input[name=pwd]", "value": ADMIN_PASS, "session": "wpadmin"})
    api("/click", {"selector": "input[name=wp-submit]", "session": "wpadmin", "wait_navigation": True})
    api("/wait", {"selector": "#wpadminbar", "ms": 10000, "session": "wpadmin"})
    who = ev("wpadmin", "(document.querySelector('#wp-admin-bar-my-account .display-name')||{textContent:''}).textContent")
    ok = who == ADMIN_USER
    check("login (wp-login.php)", ok, f"howdy {who!r}")
    if not ok:
        fail("login", "no wpadminbar display-name")
    # storageState equivalent: the cookie jar out of the browser and back
    state = api("/sessions/state", {"session": "wpadmin"})
    (HERE / "wp-state.json").write_text(json.dumps(state))
    n = len(state.get("cookies", state)) if isinstance(state, dict) else state
    check("session state export", isinstance(n, int) and n > 0, f"{n} cookies -> wp-state.json")


def journey_publish(url):
    ts = time.strftime("%H:%M:%S")
    title = f"Smoke post {ts}"
    body = ("First paragraph typed by nobody — pasted by navette. "
            f"Second paragraph: zero Chromium were downloaded making this post ({ts}).")
    api("/navigate", {"url": f"{url}/wp-admin/post-new.php", "session": "wpadmin"})
    # wait for the editor to hydrate — clicking into a half-alive editor is
    # the classic flake; the publish toggle is top-level, so /wait sees it.
    waited = api("/wait", {"selector": "button.editor-post-publish-panel__toggle",
                           "ms": 20000, "session": "wpadmin"})
    if not waited.get("ok"):
        fail("publish (editor)", "Gutenberg publish toggle never appeared")
    time.sleep(1)  # canvas iframe settles just after the chrome does
    api("/key", {"key": "Escape", "session": "wpadmin"})  # welcome guide, first boot only
    time.sleep(0.5)

    # title: contenteditable H1 inside the canvas iframe -> synthetic paste
    r = ev("wpadmin", f"""(function(){{var d={CANVAS};if(!d||!d.querySelector)return 'no-canvas';
      var el=d.querySelector('.wp-block-post-title');if(!el)return 'no-title';
      el.focus();el.innerHTML='';
      var dt=new DataTransfer();dt.setData('text/plain',{json.dumps(title)});
      el.dispatchEvent(new ClipboardEvent('paste',{{clipboardData:dt,bubbles:true,cancelable:true}}));
      return el.textContent}})()""")
    if r != title:
        fail("publish (title)", f"paste read-back {r!r}")

    # body: click the appender to spawn the first paragraph block, then paste
    ev("wpadmin", f"""(function(){{var d={CANVAS};var app=d&&d.querySelector('.block-editor-default-block-appender__content');
      if(!app)return 'no-appender';
      var o={{bubbles:true,cancelable:true,pointerId:1,pointerType:'mouse',isPrimary:true,clientX:100,clientY:200}};
      app.dispatchEvent(new PointerEvent('pointerdown',o));app.dispatchEvent(new PointerEvent('pointerup',o));
      app.dispatchEvent(new MouseEvent('click',o));return 'ok'}})()""")
    time.sleep(0.6)
    r = ev("wpadmin", f"""(function(){{var d={CANVAS};if(!d)return 'no-canvas';
      var el=(d.activeElement&&d.activeElement.closest('[data-block]'))||d.querySelector('[data-type="core/paragraph"] [contenteditable], .wp-block-paragraph [contenteditable], [data-type="core/paragraph"]');
      if(!el)return 'no-block';
      el.focus();
      var dt=new DataTransfer();dt.setData('text/plain',{json.dumps(body)});
      el.dispatchEvent(new ClipboardEvent('paste',{{clipboardData:dt,bubbles:true,cancelable:true}}));
      return 'ok'}})()""")
    time.sleep(1)
    got = ev("wpadmin", f"""(function(){{var d={CANVAS};var t=d&&d.querySelector('.wp-block-post-title');
      var p=d&&d.querySelector('[data-type="core/paragraph"]');
      return {{title:t?t.textContent:null, body:p?p.textContent.slice(0,40):null}}}})()""")
    if got.get("title") != title or not got.get("body"):
        fail("publish (compose)", f"editor holds {got}")

    api("/click", {"selector": "button.editor-post-publish-panel__toggle", "session": "wpadmin"})
    time.sleep(1.2)
    api("/click", {"selector": "button.editor-post-publish-button", "session": "wpadmin"})
    permalink = None
    for _ in range(12):
        time.sleep(1.2)
        links = ev("wpadmin", """(Array.prototype.slice.call(document.querySelectorAll(
            '.post-publish-panel a[href], .editor-post-publish-panel a[href], .components-snackbar a[href]'))
            .map(function(a){return a.href}).filter(function(h){return h.indexOf('?p=')>0||/\\/\\d{4}\\//.test(h)}))""")
        if links:
            permalink = links[0]
            break
    if not permalink:
        fail("publish (submit)", "no permalink in the success panel after 14 s")
    screenshot("wpadmin", "03-published.png")
    check("publish (block editor)", True, f"{permalink.split('//')[1]}")
    return title, body, permalink


def journey_frontend(url, title, body, permalink):
    api("/navigate", {"url": permalink, "session": "wpfront"})
    time.sleep(2)
    got = ev("wpfront", """(function(){var h=document.querySelector('h1.entry-title, h1');
      return {title:h?h.textContent.trim():null, body:document.body.innerText}})()""")
    ok_t = got.get("title") == title
    snippet = body.split(".")[1].strip()[:30]
    ok_b = snippet in str(got.get("body"))
    screenshot("wpfront", "04-frontend.png")
    check("frontend verify", ok_t and ok_b,
          f"title {'✓' if ok_t else '✗'}, body snippet {'✓' if ok_b else '✗'}")


def journey_comment(url, permalink):
    api("/navigate", {"url": permalink, "session": "wpfront"})
    time.sleep(1.5)
    for sel, val in [("textarea#comment", "Automated smoke comment — posted by navette, held for moderation."),
                     ("input#author", "navette smoke"),
                     ("input#email", "smoke@local.test")]:
        res = api("/type", {"selector": sel, "value": val, "session": "wpfront"})
        if "OK" not in str(res.get("result")):
            fail("comment", f"could not fill {sel}")
    api("/click", {"selector": "input#submit", "session": "wpfront", "wait_navigation": True})
    time.sleep(1.5)
    res = ev("wpfront", "document.body.innerText")
    txt = res if isinstance(res, str) else str(res.get("raw", ""))
    ok = "awaiting moderation" in txt.lower()
    screenshot("wpfront", "05-comment.png")
    check("comment submit", ok, "held for moderation ✓" if ok else "no moderation notice")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8090")
    args = ap.parse_args()
    print(f"== navette WordPress smoke — {args.url} via {NAVETTE} ==")
    try:
        sys.stdout.reconfigure(line_buffering=True)
    except AttributeError:
        pass

    journey_install(args.url)
    screenshot("wpadmin", "01-after-install.png")
    journey_login(args.url)
    screenshot("wpadmin", "02-dashboard.png")
    title, body, permalink = journey_publish(args.url)
    journey_frontend(args.url, title, body, permalink)
    journey_comment(args.url, permalink)

    for s in ("wpadmin", "wpfront"):
        api("/sessions/close", {"session": s})
    report()
    sys.exit(0 if all(ok for _, ok, _ in RESULTS) else 1)


if __name__ == "__main__":
    main()
