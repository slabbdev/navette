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

Every wait in here is a probe at 150-300 ms with a concrete condition —
blind sleeps are how smoke suites lose seconds and gain flakes.
"""

import argparse
import json
import os
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


def probe(session, js, want, timeout=10.0, tick=0.25):
    """Poll an evaluate expression until it equals `want` (or is truthy when
    want is None). 250 ms ticks — the navette-side /wait already proves this
    beats blind sleeps; same discipline here for what /wait cannot see
    (inside the canvas iframe, or specific values)."""
    deadline = time.time() + timeout
    got = None
    while time.time() < deadline:
        got = ev(session, js)
        hit = (bool(got) and got != {}) if want is None else got == want
        if hit:
            return got
        time.sleep(tick)
    return got


def check(step, ok, detail=""):
    RESULTS.append((step, ok, detail))
    print(f"  {'✓' if ok else '✗'} {step:<28} {detail}", flush=True)


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
    api("/navigate", {"url": f"{url}/wp-admin/", "session": "wpadmin"})
    state = ev("wpadmin", "location.pathname")
    if "/wp-login.php" in str(state):
        check("install", True, "already installed — skipped")
        return
    # Fresh boot shows a language-select step whose form submit is flaky
    # under synthetic events on CI runners. The step is optional: the same
    # page with ?language= serves the five-field install form directly —
    # what the language form would have POSTed anyway.
    if ev("wpadmin", "!!document.querySelector('select[name=language]')") is True:
        api("/navigate", {"url": f"{url}/wp-admin/install.php?language=en_US", "session": "wpadmin"})
    if probe("wpadmin", "!!document.querySelector('input[name=weblog_title]')", True, timeout=25) is not True:
        state = ev("wpadmin", "(function(){var f=document.querySelector('form');var sel=document.querySelector('select[name=language]');var opt=sel?Array.prototype.slice.call(sel.options).map(function(o){return o.value}).slice(0,3):null;return {p:location.pathname+location.search, t:document.title, form:f?(f.method+' '+f.action).slice(0,80):null, formHTML:f?f.outerHTML.slice(0,260):null, langVals:opt}})()")
        fail("install", f"install form never appeared — page: {json.dumps(state)}")
    for sel, val in [("input[name=weblog_title]", SITE_TITLE),
                     ("input[name=user_name]", ADMIN_USER),
                     ("input[name=admin_password]", ADMIN_PASS),
                     ("input[name=admin_password2]", ADMIN_PASS),
                     ("input[name=admin_email]", "smoke@local.test")]:
        res = api("/type", {"selector": sel, "value": val, "session": "wpadmin"})
        if "OK" not in str(res.get("result")):
            fail("install", f"could not fill {sel}: {res}")
    api("/click", {"selector": "input[name=Submit]", "session": "wpadmin", "wait_navigation": True})
    done = probe("wpadmin", '(document.querySelector("h1")||{textContent:""}).textContent.includes("Success")',
                 True, timeout=8)
    check("install (wizard)", bool(done), "5-minute install in 5 s" if done else "wizard did not report Success")
    if not done:
        fail("install", "wizard did not report Success")


def journey_login(url):
    api("/navigate", {"url": f"{url}/wp-login.php", "session": "wpadmin"})
    api("/type", {"selector": "input[name=log]", "value": ADMIN_USER, "session": "wpadmin"})
    api("/type", {"selector": "input[name=pwd]", "value": ADMIN_PASS, "session": "wpadmin"})
    api("/click", {"selector": "input[name=wp-submit]", "session": "wpadmin", "wait_navigation": True})
    who = probe("wpadmin", '(document.querySelector("#wp-admin-bar-my-account .display-name")||{textContent:""}).textContent',
                ADMIN_USER, timeout=10)
    check("login (wp-login.php)", who == ADMIN_USER, f"howdy {who!r}")
    if who != ADMIN_USER:
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
    waited = api("/wait", {"selector": "button.editor-post-publish-panel__toggle",
                           "ms": 20000, "session": "wpadmin"})
    if not waited.get("ok"):
        fail("publish (editor)", "Gutenberg publish toggle never appeared")
    # canvas iframe settles just after the chrome does — probe until the
    # appender exists (a live signal the writing flow is mounted), then give
    # React one commit beat. Probing only the title block fires too early and
    # the appender click gets swallowed by the tail of hydration.
    if probe("wpadmin", f"!!({CANVAS}&&{CANVAS}.querySelector('.block-editor-default-block-appender__content'))",
             True, timeout=10) is not True:
        fail("publish (editor)", "canvas iframe never exposed the appender")
    time.sleep(0.3)
    api("/key", {"key": "Escape", "session": "wpadmin"})  # welcome guide, first boot only

    # title: contenteditable H1 inside the canvas iframe -> synthetic paste
    # (read-back in the same round-trip — the paste returns what landed)
    got = ev("wpadmin", f"""(function(){{var d={CANVAS};if(!d||!d.querySelector)return 'no-canvas';
      var el=d.querySelector('.wp-block-post-title');if(!el)return 'no-title';
      el.focus();el.innerHTML='';
      var dt=new DataTransfer();dt.setData('text/plain',{json.dumps(title)});
      el.dispatchEvent(new ClipboardEvent('paste',{{clipboardData:dt,bubbles:true,cancelable:true}}));
      return el.textContent}})()""")
    if got != title:
        fail("publish (title)", f"paste read-back {got!r}")

    # body: click the appender to spawn the first paragraph block — with the
    # swallow-retry pattern (a click during a hydration re-render dies
    # silently; if the block didn't spawn, click again while it's still there)
    spawned = False
    for _ in range(3):
        ev("wpadmin", f"""(function(){{var d={CANVAS};var app=d&&d.querySelector('.block-editor-default-block-appender__content');
      if(!app)return 'no-appender';
      var o={{bubbles:true,cancelable:true,pointerId:1,pointerType:'mouse',isPrimary:true,clientX:100,clientY:200}};
      app.dispatchEvent(new PointerEvent('pointerdown',o));app.dispatchEvent(new PointerEvent('pointerup',o));
      app.dispatchEvent(new MouseEvent('click',o));return 'ok'}})()""")
        if probe("wpadmin", f"!!({CANVAS}.querySelector('[data-type=\"core/paragraph\"], .wp-block-paragraph'))",
                 True, timeout=2, tick=0.15) is True:
            spawned = True
            break
    if not spawned:
        fail("publish (appender)", "first paragraph block never spawned")
    got = ev("wpadmin", f"""(function(){{var d={CANVAS};if(!d)return 'no-canvas';
      var el=(d.activeElement&&d.activeElement.closest('[data-block]'))||d.querySelector('[data-type="core/paragraph"] [contenteditable], .wp-block-paragraph [contenteditable], [data-type="core/paragraph"]');
      if(!el)return 'no-block';
      el.focus();
      var dt=new DataTransfer();dt.setData('text/plain',{json.dumps(body)});
      el.dispatchEvent(new ClipboardEvent('paste',{{clipboardData:dt,bubbles:true,cancelable:true}}));
      var p=d.querySelector('[data-type="core/paragraph"]');
      return p?p.textContent:null}})()""")
    if not got or body.split(".")[0] not in str(got):
        fail("publish (compose)", f"editor holds {got!r}")

    api("/click", {"selector": "button.editor-post-publish-panel__toggle", "session": "wpadmin"})
    if probe("wpadmin", '(function(){var b=document.querySelector("button.editor-post-publish-button");return b&&!b.disabled})()',
             True, timeout=5, tick=0.15) is not True:
        fail("publish (panel)", "publish button never armed")
    api("/click", {"selector": "button.editor-post-publish-button", "session": "wpadmin"})
    permalink = None
    deadline = time.time() + 15
    while time.time() < deadline and not permalink:
        links = ev("wpadmin", """(Array.prototype.slice.call(document.querySelectorAll(
            '.post-publish-panel a[href], .editor-post-publish-panel a[href], .components-snackbar a[href]'))
            .map(function(a){return a.href}).filter(function(h){return h.indexOf('?p=')>0||/\\/\\d{4}\\//.test(h)}))""")
        if links:
            permalink = links[0]
            break
        time.sleep(0.25)
    if not permalink:
        fail("publish (submit)", "no permalink in the success panel after 15 s")
    screenshot("wpadmin", "03-published.png")
    check("publish (block editor)", True, f"{permalink.split('//')[1]}")
    return title, body, permalink


def journey_frontend(title, body, permalink):
    api("/navigate", {"url": permalink, "session": "wpfront"})
    got = probe("wpfront", '(function(){var h=document.querySelector("h1.entry-title, h1");return h?h.textContent.trim():null})()',
                title, timeout=10)
    page_text = ev("wpfront", "document.body.innerText")
    page_text = page_text if isinstance(page_text, str) else str(page_text.get("raw", ""))
    snippet = body.split(".")[1].strip()[:30]
    ok_b = snippet in page_text
    screenshot("wpfront", "04-frontend.png")
    check("frontend verify", got == title and ok_b,
          f"title {'✓' if got == title else '✗'}, body snippet {'✓' if ok_b else '✗'}")


def journey_comment(permalink):
    api("/navigate", {"url": permalink, "session": "wpfront"})
    if probe("wpfront", '!!document.querySelector("#comment")', True, timeout=10) is not True:
        fail("comment", "comment form never appeared")
    for sel, val in [("textarea#comment", "Automated smoke comment — posted by navette, held for moderation."),
                     ("input#author", "navette smoke"),
                     ("input#email", "smoke@local.test")]:
        res = api("/type", {"selector": sel, "value": val, "session": "wpfront"})
        if "OK" not in str(res.get("result")):
            fail("comment", f"could not fill {sel}")
    api("/click", {"selector": "input#submit", "session": "wpfront", "wait_navigation": True})
    txt = probe("wpfront", 'document.body.innerText.toLowerCase().includes("awaiting moderation")', True, timeout=10, tick=0.3)
    screenshot("wpfront", "05-comment.png")
    check("comment submit", bool(txt), "held for moderation ✓" if txt else "no moderation notice")


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
    journey_frontend(title, body, permalink)
    journey_comment(permalink)

    for s in ("wpadmin", "wpfront"):
        api("/sessions/close", {"session": s})
    report()
    sys.exit(0 if all(ok for _, ok, _ in RESULTS) else 1)


if __name__ == "__main__":
    main()
