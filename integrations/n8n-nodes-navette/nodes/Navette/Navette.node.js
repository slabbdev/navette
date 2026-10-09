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

const OPERATIONS = [
  { name: 'navigate', description: 'Open a URL', hint: 'navigate + wait for load' },
  { name: 'read', description: 'Read the page', hint: 'markdown/text content' },
  { name: 'screenshot', description: 'Screenshot', hint: 'PNG, binary output' },
  { name: 'click', description: 'Click', hint: 'pointer+mouse events' },
  { name: 'type', description: 'Type into a field', hint: 'React-safe setter' },
  { name: 'evaluate', description: 'Run JavaScript', hint: 'anything the others miss' },
  { name: 'wait', description: 'Wait for a selector', hint: 'polls until it exists' },
  { name: 'login', description: 'Login (keychain credential)', hint: 'the agent never sees the password' },
  { name: 'exportState', description: 'Export cookie state', hint: 'the logged-in jar' },
  { name: 'importState', description: 'Import cookie state', hint: 'restore a login' },
  { name: 'viewport', description: 'Set viewport size', hint: 'width × height, affects rendering and screenshots' },
  { name: 'sessions', description: 'List sessions', hint: 'url + title each' },
  { name: 'closeSession', description: 'Close a session', hint: 'free the window' },
];

const OPERATION_FIELD = {
  displayName: 'Operation',
  name: 'operation',
  type: 'options',
  noExpression: true,
  required: true,
  default: 'navigate',
  options: OPERATIONS.map((o) => ({ name: o.name, value: o.name, description: o.description, action: o.hint })),
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
      subtitle: '={{$parameter["operation"]}}',
      description: 'The browser for agents — drive a real system WebView (navigate, read, see, act)',
      defaults: { name: 'navette' },
      inputs: ['main'],
      outputs: ['main'],
      credentials: [{ name: 'navetteApi', required: true }],
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
          ...show('navigate', 'read', 'screenshot', 'click', 'type', 'evaluate', 'wait', 'viewport', 'login', 'exportState', 'importState', 'closeSession'),
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
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers.Authorization = `Bearer ${token}`;

    const out = [];
    for (let i = 0; i < items.length; i++) {
      const operation = this.getNodeParameter('operation', i);
      const session = this.getNodeParameter('session', i) || 'default';
      let path = null;
      let body = { session };

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
