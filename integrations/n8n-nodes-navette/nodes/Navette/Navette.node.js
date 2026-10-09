'use strict';

/**
 * navette — the browser for agents, as an n8n node.
 *
 * Wraps the daemon's loopback HTTP API (127.0.0.1, JSON). No SDK, no
 * dependencies: every operation is one POST. Sessions are named browser
 * contexts that live in the daemon — use one session per site and the
 * cookie state survives between nodes.
 *
 * The pair that unlocks "sites with no API": Export State / Import State
 * (the cookie jar out of the browser and back in) and Login (the daemon's
 * origin-bound keychain credential — the workflow never sees the password).
 */

// Beginner-first naming: the label is what the user picks, the description
// is the one line they see in the actions list. Values are stable — never
// rename a value.
const OPERATIONS = [
  { name: 'Open a page', value: 'navigate', description: 'Open a website — waits for it to load' },
  { name: 'Read the page', value: 'read', description: 'Get the page text (markdown)' },
  { name: 'Take a screenshot', value: 'screenshot', description: 'Capture the page as a PNG image' },
  { name: 'Click something', value: 'click', description: 'Click a button or a link' },
  { name: 'Fill a field', value: 'type', description: 'Type text into a form field' },
  { name: 'Wait for something', value: 'wait', description: 'Wait until an element shows up' },
  { name: 'Run JavaScript', value: 'evaluate', description: 'Advanced: run your own code in the page' },
  { name: 'Browse for me (AI)', value: 'agent', description: 'Give a mission in plain words — the node drives the browser by itself' },
  { name: 'Log in (saved password)', value: 'login', description: 'Log in with a password saved in the system keychain — the workflow never sees it' },
  { name: 'Save login session', value: 'exportState', description: 'Export the logged-in cookies for later' },
  { name: 'Restore login session', value: 'importState', description: 'Reuse saved cookies — no login needed' },
  { name: 'Set window size', value: 'viewport', description: 'Choose the page size, e.g. 1920×1080' },
  { name: 'List open pages', value: 'sessions', description: 'See what is currently open' },
  { name: 'Close a page', value: 'closeSession', description: 'Close the browser window' },
];

const OPERATION_FIELD = {
  displayName: 'Operation',
  name: 'operation',
  type: 'options',
  noExpression: true,
  required: true,
  default: 'navigate',
  options: OPERATIONS.map((o) => ({ name: o.name, value: o.value, description: o.description })),
};

const show = (...ops) => ({ displayOptions: { show: { operation: ops } } });

function field(displayName, name, type, extra = {}) {
  return { displayName, name, type, ...extra };
}

class Navette {
  constructor() {
    this.description = {
      displayName: 'navette',
      name: 'navette',
      icon: 'file:navette.svg',
      group: ['transform'],
      version: 1,
      subtitle: '={{ {"navigate":"Open a page","read":"Read the page","screenshot":"Take a screenshot","click":"Click something","type":"Fill a field","evaluate":"Run JavaScript","wait":"Wait for something","login":"Log in","exportState":"Save login session","importState":"Restore login session","viewport":"Set window size","sessions":"List open pages","closeSession":"Close a page"}[$parameter["operation"]] }}',
      description: 'Drive a real browser: open websites, click, fill forms, take screenshots. Works on its own — or as a tool for the AI Agent node, so the agent browses by itself.',
      defaults: { name: 'navette' },
      usableAsTool: true,
      inputs: ['main'],
      outputs: ['main'],
      credentials: [
        { name: 'navetteApi', required: false },
        { name: 'navetteLlmApi', required: false, displayOptions: { show: { operation: ['agent'] } } },
      ],
      properties: [
        OPERATION_FIELD,

        field('URL', 'url', 'string', {
          ...show('navigate'),
          required: true,
          placeholder: 'https://example.com',
          description: 'Page to open (navigations that redirect complete fine)',
        }),
        field('Also Return Content', 'withContent', 'boolean', {
          ...show('navigate'),
          default: true,
          description: 'Fold the page read into the same round-trip',
        }),
        field('Format', 'format', 'options', {
          ...show('navigate', 'read'),
          options: [
            { name: 'Markdown', value: 'markdown' },
            { name: 'Text', value: 'text' },
            { name: 'HTML', value: 'html' },
          ],
          default: 'markdown',
        }),

        // ---- Navigate options: the one-node journey (viewport → wait →
        // screenshot happen inside Navigate, so a basic capture is ONE node)
        field('Options', 'navigateOptions', 'collection', {
          ...show('navigate'),
          label: 'Options',
          typeOptions: {
            multipleValues: false,
          },
          options: [
            field('Viewport', 'viewport', 'fixedCollection', {
              typeOptions: { multipleValues: false },
              options: [
                { name: 'values', displayName: 'Viewport', values: [
                  field('Width', 'width', 'number', { default: 1920, description: 'Viewport width (px)' }),
                  field('Height', 'height', 'number', { default: 1080, description: 'Viewport height (px)' }),
                ]},
              ],
              default: {},
              description: 'Set before the page loads — screenshots come out at this size',
            }),
            field('Wait For Selector', 'waitFor', 'string', {
              placeholder: 'textarea[name=q], #main, [data-testid=result]',
              description: 'Wait until this CSS selector exists before continuing',
            }),
            field('Max Wait (ms)', 'waitForMs', 'number', { default: 15000, description: 'Fail if the selector never appears' }),
            field('Screenshot', 'screenshot', 'boolean', { default: false, description: 'Capture a PNG after the page settles — binary output "screenshot"' }),
          ],
          default: {},
          description: 'Viewport, wait-for-selector, screenshot — the full journey in this one node',
        }),

        field('Selector', 'selector', 'string', {
          ...show('click', 'type', 'wait'),
          required: true,
          placeholder: 'button.submit, input[name=q], a[href*=login]',
          description: 'CSS selector of the element to act on',
        }),
        field('Value', 'value', 'string', {
          ...show('type'),
          description: 'Text to type (native prototype setter + input/change events — React-safe)',
        }),
        field('Wait For Navigation', 'waitNavigation', 'boolean', {
          ...show('click'),
          default: false,
          description: 'The click submits a form / follows a link — wait for the new page',
        }),
        field('Max Wait (ms)', 'ms', 'number', {
          ...show('wait'),
          default: 10000,
          description: 'Fail if the selector never appears',
        }),

        field('JavaScript', 'js', 'string', {
          ...show('evaluate'),
          required: true,
          typeOptions: { editor: 'jsEditor' },
          description: 'Expression evaluated in the page; return a string or JSON.stringify objects',
        }),

        field('Site', 'site', 'string', {
          ...show('login'),
          required: true,
          placeholder: 'mywp',
          description: 'A credential registered with `navette creds set SITE` — the daemon fills it on the bound origin, the password never leaves the keychain',
        }),

        field('Cookies (JSON)', 'cookies', 'string', {
          ...show('importState'),
          required: true,
          typeOptions: { editor: 'jsonEditor' },
          description: 'The jar exported by Export State (raw or {"cookies":[…]})',
        }),

        field('Mission', 'mission', 'string', {
          ...show('agent'),
          // NOT `required` in the schema: n8n validates required params
          // BEFORE the trigger data flows, and an expression like
          // {{$json.chatInput}} evaluates empty at that moment — the check
          // would block every chat run. Runtime check below instead.
          typeOptions: { editor: 'textEditor' },
          placeholder: 'e.g. Go to news.ycombinator.com and give me the top 5 titles with their points',
          description: 'What you want, in plain words. The node opens the page, clicks, fills, reads — by itself.',
        }),
        field('Start URL (optional)', 'startUrl', 'string', {
          ...show('agent'),
          placeholder: 'https://…  (leave empty to let the agent choose)',
          description: 'Page to open first, if any',
        }),
        field('Max Steps', 'maxSteps', 'number', {
          ...show('agent'),
          default: 8,
          description: 'Safety cap on browser actions the agent may take',
        }),

        field('Width', 'width', 'number', {
          ...show('viewport'),
          default: 1920,
          description: 'Viewport width in pixels',
        }),
        field('Height', 'height', 'number', {
          ...show('viewport'),
          default: 1080,
          description: 'Viewport height in pixels',
        }),

        field('Session', 'session', 'string', {
          ...show('navigate', 'read', 'screenshot', 'click', 'type', 'evaluate', 'wait', 'viewport', 'agent', 'login', 'exportState', 'importState', 'closeSession'),
          default: '',
          placeholder: 'default',
          description: 'Named browser context in the daemon — keep one per site; cookies persist between nodes',
        }),
      ],
    };
  }

  async execute() {
    const items = this.getInputData();
    let base = 'http://127.0.0.1:8765';
    let token = '';
    try {
      const creds = await this.getCredentials('navetteApi');
      if (creds) {
        base = (creds.baseUrl || base).replace(/\/+$/, '');
        token = creds.token || '';
      }
    } catch {
      // no credential configured — the loopback default stands (a daemon on
      // 8765 without --token needs nothing)
    }
    let llm = null;
    try {
      const c = await this.getCredentials('navetteLlmApi');
      if (c && c.apiKey) {
        llm = {
          base: (c.baseUrl || 'https://generativelanguage.googleapis.com/v1beta/openai').replace(/\/+$/, ''),
          key: c.apiKey,
          model: c.model || 'gemini-2.5-flash',
        };
      }
    } catch {}
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers.Authorization = `Bearer ${token}`;

    const out = [];
    for (let i = 0; i < items.length; i++) {
      const operation = this.getNodeParameter('operation', i);
      const session = this.getNodeParameter('session', i) || 'default';
      let path = null;
      let body = { session };

      // ---- Browse for me (AI): the self-driving journey -------------------
      if (operation === 'agent') {
        if (!llm) {
          throw new Error('Browse for me needs an LLM: open the node credentials and configure "navette agent — LLM" (a free Gemini key works).');
        }
        const mission = this.getNodeParameter('mission', i);
        if (!mission || !String(mission).trim()) {
          throw new Error('Browse for me needs a Mission — what should the agent do? (e.g. "Go to example.com and tell me the page title")');
        }
        const startUrl = this.getNodeParameter('startUrl', i) || '';
        const maxSteps = Math.min(this.getNodeParameter('maxSteps', i) || 8, 20);

        const daemon = async (p, b) => {
          const r = await fetch(`${base}${p}`, { method: 'POST', headers, body: JSON.stringify({ session, ...b }) });
          const text = await r.text();
          let j; try { j = JSON.parse(text); } catch { j = { raw: text.slice(0, 500) }; }
          return j;
        };

        const messages = [
          { role: 'system', content:
            'You drive a real browser through the provided tools. Method: open the page, read the markdown content returned by open_page, then click/fill/run_js as needed until the mission is fulfilled. ' +
            'Selectors are CSS (inspect with run_js if unsure: document.querySelectorAll). ' +
            'When the mission is done, call finish with the answer, in the user\'s language, concise.' },
          { role: 'user', content: 'Mission: ' + mission + (startUrl ? `\nStart by opening: ${startUrl}` : '') },
        ];
        const tools = [
          { type: 'function', function: { name: 'open_page', description: 'Open a URL; returns the page title and content as markdown — this is how you read the web', parameters: { type: 'object', properties: { url: { type: 'string', description: 'Full URL' } }, required: ['url'] } } },
          { type: 'function', function: { name: 'click', description: 'Click an element by CSS selector', parameters: { type: 'object', properties: { selector: { type: 'string' } }, required: ['selector'] } } },
          { type: 'function', function: { name: 'fill', description: 'Type text into a field by CSS selector', parameters: { type: 'object', properties: { selector: { type: 'string' }, text: { type: 'string' } }, required: ['selector', 'text'] } } },
          { type: 'function', function: { name: 'run_js', description: 'Run JavaScript in the page; return JSON.stringify(...) of what you want back', parameters: { type: 'object', properties: { expression: { type: 'string' } }, required: ['expression'] } } },
          { type: 'function', function: { name: 'screenshot', description: 'Capture the current page as a PNG (kept in the node output)', parameters: { type: 'object', properties: {} } } },
          { type: 'function', function: { name: 'finish', description: 'The mission is complete — deliver the final answer', parameters: { type: 'object', properties: { answer: { type: 'string', description: 'The final answer for the user' } }, required: ['answer'] } } },
        ];

        let answer = null;
        const log = [];
        let shotBytes = null;
        let lastShot = null;
        for (let step = 0; step < maxSteps && !answer; step++) {
          const chat = await fetch(`${llm.base}/chat/completions`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${llm.key}` },
            body: JSON.stringify({ model: llm.model, messages, tools, tool_choice: 'auto' }),
          });
          if (!chat.ok) {
            throw new Error(`LLM error (HTTP ${chat.status}): ${(await chat.text()).slice(0, 300)}`);
          }
          const choice = (await chat.json()).choices?.[0]?.message;
          if (!choice) throw new Error('LLM returned no message');
          messages.push(choice);

          const calls = choice.tool_calls || [];
          if (!calls.length) {
            answer = choice.content || '(the model returned no answer)';
            log.push({ step, action: 'answer', detail: String(answer).slice(0, 200) });
            break;
          }
          for (const call of calls) {
            const fn = call.function?.name;
            let args = {};
            try { args = JSON.parse(call.function?.arguments || '{}'); } catch {}
            let result;
            if (fn === 'open_page') {
              const r = await daemon('/navigate', { url: args.url, with_content: true, format: 'markdown' });
              result = r.ok ? `TITLE: ${r.title}\n\n${String(r.content || '').slice(0, 6000)}` : `ERROR: ${JSON.stringify(r).slice(0, 300)}`;
              log.push({ step, action: 'open_page', url: args.url, ok: !!r.ok });
            } else if (fn === 'click') {
              const r = await daemon('/click', { selector: args.selector, wait_navigation: true });
              result = JSON.stringify(r).slice(0, 300);
              log.push({ step, action: 'click', selector: args.selector });
            } else if (fn === 'fill') {
              const r = await daemon('/type', { selector: args.selector, value: args.text });
              result = JSON.stringify(r).slice(0, 300);
              log.push({ step, action: 'fill', selector: args.selector });
            } else if (fn === 'run_js') {
              const r = await daemon('/evaluate', { js: args.expression });
              result = (r.result || JSON.stringify(r)).slice(0, 3000);
              log.push({ step, action: 'run_js', detail: args.expression?.slice(0, 80) });
            } else if (fn === 'screenshot') {
              const sr = await fetch(`${base}/screenshot`, { method: 'POST', headers, body: JSON.stringify({ session }) });
              const buf = Buffer.from(await sr.arrayBuffer());
              shotBytes = buf.length;
              result = `Screenshot captured (${buf.length} bytes) — it is attached to the node output.`;
              log.push({ step, action: 'screenshot' });
              lastShot = buf;
            } else if (fn === 'finish') {
              answer = args.answer || 'Done.';
              log.push({ step, action: 'finish' });
              break;
            } else {
              result = `Unknown tool: ${fn}`;
            }
            messages.push({ role: 'tool', tool_call_id: call.id, content: String(result) });
          }
        }
        if (!answer) answer = `Stopped after ${maxSteps} steps without finishing — the log shows what happened.`;

        const itemJson = { ok: true, answer, steps: log };
        if (lastShot) {
          const binary = await this.helpers.prepareBinaryData(lastShot, 'agent-screenshot.png', 'image/png');
          out.push({ json: itemJson, binary: { data: binary } });
        } else {
          out.push({ json: itemJson });
        }
        continue;
      }

      switch (operation) {
        case 'navigate':
          body.url = this.getNodeParameter('url', i);
          body.with_content = this.getNodeParameter('withContent', i);
          body.format = this.getNodeParameter('format', i);
          path = '/navigate';
          break;
        case 'read':
          body.format = this.getNodeParameter('format', i);
          path = '/read';
          break;
        case 'screenshot':
          path = '/screenshot';
          break;
        case 'click':
          body.selector = this.getNodeParameter('selector', i);
          body.wait_navigation = this.getNodeParameter('waitNavigation', i);
          path = '/click';
          break;
        case 'type':
          body.selector = this.getNodeParameter('selector', i);
          body.value = this.getNodeParameter('value', i);
          path = '/type';
          break;
        case 'evaluate':
          body.js = this.getNodeParameter('js', i);
          path = '/evaluate';
          break;
        case 'wait':
          body.selector = this.getNodeParameter('selector', i);
          body.ms = this.getNodeParameter('ms', i);
          path = '/wait';
          break;
        case 'login':
          body.site = this.getNodeParameter('site', i);
          path = '/login';
          break;
        case 'exportState':
          path = '/sessions/state';
          break;
        case 'importState': {
          const raw = this.getNodeParameter('cookies', i);
          let parsed;
          try {
            parsed = typeof raw === 'string' ? JSON.parse(raw) : raw;
          } catch (e) {
            throw new Error(`Cookies is not valid JSON: ${e.message}`);
          }
          body.cookies = parsed && parsed.cookies ? parsed.cookies : parsed;
          path = '/sessions/load';
          break;
        }
        case 'viewport':
          body.width = this.getNodeParameter('width', i);
          body.height = this.getNodeParameter('height', i);
          path = '/sessions/viewport';
          break;
        case 'sessions':
          path = null; // GET
          break;
        case 'closeSession':
          path = '/sessions/close';
          break;
        default:
          throw new Error(`Unknown operation: ${operation}`);
      }

      const url = path === null ? `${base}/sessions` : `${base}${path}`;
      const method = path === null ? 'GET' : 'POST';
      const call = async (p, b) => {
        const r = await fetch(`${base}${p}`, {
          method: 'POST',
          headers,
          body: JSON.stringify(b),
        });
        if (!r.ok) {
          throw new Error(`navette ${p} failed (HTTP ${r.status}): ${await r.text()}`);
        }
        return r;
      };

      // ---- navigate with Options: the one-node journey -------------------
      // viewport (before the page loads) → navigate → wait-for-selector →
      // screenshot. Each option is optional; with none set this is a plain
      // navigate, exactly like before.
      if (operation === 'navigate') {
        const opts = this.getNodeParameter('navigateOptions', i) || {};
        const vp = opts.viewport?.values || opts.viewport || null;

        if (vp && vp.width && vp.height) {
          await call('/sessions/viewport', { session, width: vp.width, height: vp.height });
        }

        const navRes = await call('/navigate', body);
        const navJson = await navRes.json();

        if (opts.waitFor) {
          await call('/wait', { session, selector: opts.waitFor, ms: opts.waitForMs || 15000 });
        }

        if (opts.screenshot) {
          const shotRes = await call('/screenshot', { session });
          const buf = Buffer.from(await shotRes.arrayBuffer());
          const binary = await this.helpers.prepareBinaryData(buf, 'screenshot.png', 'image/png');
          out.push({ json: { ...navJson, screenshotBytes: buf.length }, binary: { data: binary } });
          continue;
        }
        out.push({ json: navJson });
        continue;
      }

      const res = await fetch(url, {
        method,
        headers,
        body: method === 'GET' ? undefined : JSON.stringify(body),
      });

      if (operation === 'screenshot') {
        if (!res.ok) {
          throw new Error(`navette screenshot failed (HTTP ${res.status}): ${await res.text()}`);
        }
        const buf = Buffer.from(await res.arrayBuffer());
        const binary = await this.helpers.prepareBinaryData(buf, 'screenshot.png', 'image/png');
        out.push({ json: { ok: true, session, bytes: buf.length }, binary: { data: binary } });
        continue;
      }

      const text = await res.text();
      let json;
      try {
        json = JSON.parse(text);
      } catch {
        throw new Error(`navette ${operation} returned non-JSON (HTTP ${res.status}): ${text.slice(0, 200)}`);
      }
      if (!res.ok || json.ok === false) {
        throw new Error(`navette ${operation} failed (HTTP ${res.status}): ${JSON.stringify(json).slice(0, 300)}`);
      }
      out.push({ json });
    }

    return [out];
  }
}

module.exports = { Navette };
