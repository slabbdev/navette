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
      credentials: [{ name: 'navetteApi', required: false }],
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

        field('Session', 'session', 'string', {
          ...show('navigate', 'read', 'screenshot', 'click', 'type', 'evaluate', 'wait', 'login', 'exportState', 'importState', 'closeSession'),
          default: '',
          placeholder: 'default',
          description: 'Named browser context in the daemon — keep one per site; cookies persist between nodes',
        }),
      ],
    };
  }

  async execute() {
    const items = this.getInputData();
    const creds = (await this.getCredentials('navetteApi')) || {};
    const base = (creds.baseUrl || 'http://127.0.0.1:8765').replace(/\/+$/, '');
    const headers = { 'Content-Type': 'application/json' };
    if (creds.token) headers.Authorization = `Bearer ${creds.token}`;

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
