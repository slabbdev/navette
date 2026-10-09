#!/usr/bin/env node
/* The Playwright twin of wp-smoke.py — same journeys, same disposable
 * WordPress, Playwright used at its best (fill, auto-wait, frameLocator,
 * storageState). Run from bench/ so require('playwright') resolves:
 *
 *   cd ../../bench && node ../demo/wordpress/wp-smoke-playwright.js
 */
const fs = require('fs');
const path = require('path');
const { chromium } = require(path.join(__dirname, '..', '..', 'bench', 'node_modules', 'playwright'));

const URL = process.env.WP_URL || 'http://127.0.0.1:8090';
const ART = path.join(__dirname, 'artifacts-playwright');
fs.mkdirSync(ART, { recursive: true });

const results = [];
const T0 = Date.now();
function check(step, ok, detail = '') {
  results.push([step, ok]);
  console.log(`  ${ok ? '✓' : '✗'} ${step.padEnd(28)} ${detail}`);
}
async function report() {
  const passed = results.filter(r => r[1]).length;
  console.log(`\n${'='.repeat(62)}\n  ${passed}/${results.length} journeys passed in ${((Date.now() - T0) / 1000).toFixed(1)} s — Playwright + Chromium`);
  return passed === results.length;
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  page.setDefaultTimeout(20000);

  // 1. install wizard (skip if already installed)
  await page.goto(`${URL}/wp-admin/`);
  if (!page.url().includes('wp-login.php')) {
    if (await page.locator('#language-continue').count()) {
      await page.click('#language-continue');
      await page.waitForSelector('input[name=weblog_title]');
    }
    await page.fill('input[name=weblog_title]', 'navette Smoke');
    await page.fill('input[name=user_name]', 'smokeadmin');
    await page.fill('input[name=admin_password]', 'navette-smoke-2026!');
    // WordPress hides the confirm field when the password is strong enough —
    // Playwright's actionability check refuses to fill it while hidden
    // (navette's synthetic /type fills it either way).
    if (await page.locator('input[name=admin_password2]').isVisible()) {
      await page.fill('input[name=admin_password2]', 'navette-smoke-2026!');
    }
    await page.fill('input[name=admin_email]', 'smoke@local.test');
    await page.click('input[name=Submit]');
    await page.waitForSelector('h1:has-text("Success")');
    check('install (wizard)', true, '5-minute install');
  } else {
    check('install', true, 'already installed — skipped');
  }
  await page.screenshot({ path: path.join(ART, '01-after-install.png') });

  // 2. login + storageState (the feature the WP guide leans on)
  await page.goto(`${URL}/wp-login.php`);
  await page.fill('input[name=log]', 'smokeadmin');
  await page.fill('input[name=pwd]', 'navette-smoke-2026!');
  await page.click('input[name=wp-submit]');
  await page.waitForSelector('#wpadminbar');
  const howdy = await page.locator('#wp-admin-bar-my-account .display-name').first().textContent();
  check('login (wp-login.php)', howdy === 'smokeadmin', `howdy ${JSON.stringify(howdy)}`);
  fs.writeFileSync(path.join(__dirname, 'wp-state-playwright.json'),
                   JSON.stringify(await ctx.storageState()));
  check('session state export', true, 'storageState() -> wp-state-playwright.json');
  await page.screenshot({ path: path.join(ART, '02-dashboard.png') });

  // 3. publish in the block editor (canvas iframe)
  const ts = new Date().toTimeString().slice(0, 8);
  const title = `Smoke post ${ts}`;
  const body = `First paragraph typed by nobody — pasted by Playwright. Second paragraph: one hundred eighty-two MB were downloaded making this post (${ts}).`;
  await page.goto(`${URL}/wp-admin/post-new.php`);
  await page.waitForSelector('.editor-post-publish-panel__toggle');
  // welcome guide (first boot): the modal overlay intercepts pointer events —
  // dismiss it AFTER the editor is up, then wait for it to be gone.
  if (await page.locator('.components-modal__screen-overlay').count()) {
    await page.locator('.components-modal__header button').first().click();
    await page.waitForSelector('.components-modal__screen-overlay', { state: 'detached' });
  }
  const canvas = page.frameLocator('iframe[name="editor-canvas"]');
  const titleBox = canvas.locator('.wp-block-post-title');
  await titleBox.click();
  await titleBox.fill(title);
  await canvas.locator('.block-editor-default-block-appender__content').click();
  await page.keyboard.type(body);
  await page.click('button.editor-post-publish-panel__toggle');
  await page.click('button.editor-post-publish-button');
  await page.waitForSelector('.components-snackbar:has-text("Post published")', { timeout: 30000 });
  const href = await page.locator('.post-publish-panel a[href*="?p="], .editor-post-publish-panel a[href*="?p="]').first().getAttribute('href');
  check('publish (block editor)', !!href, href);
  await page.screenshot({ path: path.join(ART, '03-published.png') });

  // 4+5. frontend + comment as an ANONYMOUS visitor (fresh context — the
  // admin context is logged in, and WordPress hides name/email for known users)
  const front = await browser.newContext();
  const fpage = await front.newPage();
  fpage.setDefaultTimeout(20000);
  await fpage.goto(href);
  const h1 = await fpage.locator('h1.entry-title, h1').first().textContent();
  const pageText = await fpage.locator('body').innerText();
  check('frontend verify', h1.trim() === title && pageText.includes('eighty-two MB'),
        `title ${h1.trim() === title ? '✓' : '✗'}, body snippet ${pageText.includes('eighty-two MB') ? '✓' : '✗'}`);
  await fpage.screenshot({ path: path.join(ART, '04-frontend.png') });

  await fpage.fill('#comment', 'Automated smoke comment — posted by Playwright, held for moderation.');
  await fpage.fill('#author', 'playwright smoke');
  await fpage.fill('#email', 'smoke@local.test');
  await fpage.click('#submit');
  await fpage.waitForSelector('.comment-awaiting-moderation');
  check('comment submit', true, 'held for moderation ✓');
  await fpage.screenshot({ path: path.join(ART, '05-comment.png') });

  await front.close();
  await browser.close();
  process.exit(await report() ? 0 : 1);
})().catch(async e => {
  console.error('FAIL:', e.message);
  await report();
  process.exit(1);
});
