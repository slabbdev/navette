// navbench Playwright driver — one process per measurement mode.
// Usage: node pw_target.mjs <mode> <url-template-or-url> [n]
// Modes: coldstart | navread | act | crawl
// Set CDP_URL env to connect to an external CDP endpoint (Lightpanda) instead
// of launching Chromium.

const t0 = Date.now();
const mode = process.argv[2];
const url = process.argv[3];
const n = parseInt(process.argv[4] || '1', 10);
const CDP = process.env.CDP_URL;

const { chromium } = await import('playwright');

let browser;
if (CDP) {
  let ok = false;
  for (let i = 0; i < 60 && !ok; i++) {
    try { browser = await chromium.connectOverCDP(CDP); ok = true; }
    catch { await new Promise(r => setTimeout(r, 250)); }
  }
  if (!ok) { console.log(JSON.stringify({ error: 'CDP connect failed: ' + CDP })); process.exit(1); }
} else {
  browser = await chromium.launch();
}
const launchMs = Date.now() - t0;

const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
const page = await ctx.newPage();
const out = { launchMs };

if (mode === 'coldstart') {
  await page.goto(url, { waitUntil: 'load', timeout: 30000 });
  const text = await page.evaluate(() => document.body.innerText);
  out.navreadMs = Date.now() - t0 - launchMs;
  out.len = text.length;
} else if (mode === 'navread') {
  const times = [];
  for (let i = 0; i < n; i++) {
    const u = url.replace('{i}', String(i).padStart(3, '0'));
    const s = Date.now();
    await page.goto(u, { waitUntil: 'load', timeout: 30000 });
    await page.evaluate(() => document.body.innerText);
    times.push(Date.now() - s);
  }
  out.times = times;
} else if (mode === 'act') {
  await page.goto(url, { waitUntil: 'load', timeout: 30000 });
  const times = [];
  for (let i = 0; i < n; i++) {
    const s = Date.now();
    await page.fill('#name', 'agent ' + i);
    await page.click('#go');
    times.push(Date.now() - s);
  }
  out.times = times;
} else if (mode === 'crawl') {
  const s = Date.now();
  let done = 0;
  for (let i = 0; i < n; i++) {
    const u = url.replace('{i}', String(i).padStart(3, '0'));
    await page.goto(u, { waitUntil: 'load', timeout: 30000 });
    await page.evaluate(() => document.body.innerText);
    done++;
  }
  out.wallMs = Date.now() - s;
  out.done = done;
} else {
  out.error = 'unknown mode ' + mode;
}

console.log(JSON.stringify(out));
await browser.close();
process.exit(0);
