#!/usr/bin/env node
/* Unit-verification of the node against a REAL daemon — no n8n needed.
 * Usage: NAVETTE=http://127.0.0.1:8765 node test-node.js
 * Exercises every operation incl. the screenshot binary path; exits non-zero
 * on the first failure. This ran green (8 ops / 713 ms) against a live
 * daemon during development; keep it as the package's smoke test. */
'use strict';
const { Navette } = require('./nodes/Navette/Navette.node.js');
const BASE = process.env.NAVETTE || 'http://127.0.0.1:8765';
const node = new Navette();
let current = null;
const bin = [];
const ctx = {
  getInputData: () => [{ json: {} }],
  getNodeParameter: (name) => current[name],
  getCredentials: async () => ({ baseUrl: BASE }),
  helpers: { prepareBinaryData: async (buf, name, mime) => { bin.push({ name, mime, bytes: buf.length }); return { fileName: name, mimeType: mime }; } },
};
const run = (op, p) => { current = { operation: op, session: 'n8n-test', ...p }; return node.execute.call(ctx); };
(async () => {
  const t0 = Date.now();
  const nav = (await run('navigate', { url: 'https://example.com/', withContent: false, format: 'markdown' }))[0][0].json;
  if (!nav.ok || nav.title !== 'Example Domain') throw new Error('navigate: ' + JSON.stringify(nav));
  console.log('✓ navigate');
  const ev = (await run('evaluate', { js: 'JSON.stringify({t:document.title, p:document.querySelectorAll("p").length})' }))[0][0].json;
  if (!ev.result.includes('Example Domain')) throw new Error('evaluate: ' + JSON.stringify(ev));
  console.log('✓ evaluate');
  if (!(await run('wait', { selector: 'body', ms: 3000 }))[0][0].json.ok) throw new Error('wait');
  console.log('✓ wait');
  const ss = (await run('screenshot', {}));
  if (!bin[0] || bin[0].bytes < 1000) throw new Error('screenshot binary missing');
  console.log('✓ screenshot (' + bin[0].bytes + ' bytes, binary output)');
  const jar = (await run('exportState', {}))[0][0].json;
  console.log('✓ exportState (' + ((jar.cookies || []).length) + ' cookies)');
  if (!(await run('importState', { cookies: JSON.stringify(jar) }))[0][0].json.ok) throw new Error('importState');
  console.log('✓ importState');
  if (!(await run('sessions', {}))[0][0].json.sessions) throw new Error('sessions');
  console.log('✓ sessions');
  if (!(await run('closeSession', {}))[0][0].json.ok) throw new Error('closeSession');
  console.log('✓ closeSession');
  console.log('ALL OPS OK in ' + (Date.now() - t0) + 'ms');
})().catch((e) => { console.error('FAIL:', e.message); process.exit(1); });
