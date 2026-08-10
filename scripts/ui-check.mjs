// Phase 4 UI acceptance check driver.
// Self-contained: starts Vite, drives headless Edge via CDP to capture the
// snapshot matrix from the UI spec (section 10), measures the filmstrip
// overflow constraint, then shuts everything down.
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import http from 'node:http';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const EDGE =
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const OUT_DIR = join(ROOT, 'artifacts', 'ui-check');
const DEV_URL = 'http://127.0.0.1:1420';
const DEBUG_PORT = 9333 + (process.pid % 500);
const START_MS = Date.now();
const DEADLINE_MS = 280000;

const report = {
  generatedAt: new Date().toISOString(),
  screenshots: [],
  measurements: [],
  failures: [],
};

function log(message) {
  process.stdout.write(`[ui-check] ${message}\n`);
}

function deadlineCheck() {
  if (Date.now() - START_MS > DEADLINE_MS) {
    throw new Error('global deadline exceeded');
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(check, timeoutMs, label) {
  const start = Date.now();
  for (;;) {
    deadlineCheck();
    if (await check()) return;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`timeout waiting for ${label}`);
    }
    await sleep(250);
  }
}

function httpGetJson(url) {
  return new Promise((resolve, reject) => {
    const req = http.get(url, (res) => {
      let body = '';
      res.on('data', (chunk) => (body += chunk));
      res.on('end', () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    req.on('error', reject);
    req.setTimeout(3000, () => req.destroy(new Error(`timeout GET ${url}`)));
  });
}

async function httpReady(url) {
  try {
    const res = await new Promise((resolve, reject) => {
      const req = http.get(url, (r) => resolve(r));
      req.on('error', reject);
      req.setTimeout(2500, () => req.destroy(new Error('timeout')));
    });
    res.resume();
    return res.statusCode === 200;
  } catch {
    return false;
  }
}

class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    this.loaded = false;
  }

  async open() {
    await new Promise((resolve, reject) => {
      this.ws.addEventListener('open', resolve, { once: true });
      this.ws.addEventListener('error', () => reject(new Error('ws error')), {
        once: true,
      });
    });
    this.ws.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== undefined) {
        const entry = this.pending.get(message.id);
        if (entry) {
          this.pending.delete(message.id);
          if (message.error) entry.reject(new Error(message.error.message));
          else entry.resolve(message.result);
        }
      } else if (message.method === 'Page.loadEventFired') {
        this.loaded = true;
      }
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* noop */
    }
  }
}

async function evaluate(cdp, expression) {
  const result = await cdp.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) {
    throw new Error(
      `evaluate failed: ${JSON.stringify(result.exceptionDetails.exception ?? result.exceptionDetails)}`,
    );
  }
  return result.result?.value;
}

const SETTINGS_CLICK = `(async () => {
  const btns = [...document.querySelectorAll('button')];
  const hit = btns.find((b) => {
    const text = (b.getAttribute('aria-label') || '') + ' ' + (b.textContent || '');
    return text.includes('\u8bbe\u7f6e');
  });
  if (!hit) return 'settings-button-not-found';
  hit.click();
  await new Promise((r) => setTimeout(r, 1200));
  return 'settings-opened';
})()`;

function sidebarClick(label) {
  return `(async () => {
  const btns = [...document.querySelectorAll('.tool-sidebar button')];
  const hit = btns.find((b) => (b.textContent || '').includes(${JSON.stringify(label)}));
  if (!hit) return 'sidebar-${label}-not-found';
  hit.click();
  await new Promise((r) => setTimeout(r, 900));
  return 'sidebar-${label}-opened';
})()`;
}

const PENDING_CLICK = `(async () => {
  const btns = [...document.querySelectorAll('button')];
  const hit = btns.find((b) => /\\u5f85\\u5220\\u9664\\s*\\d+/.test(b.textContent || ''));
  if (!hit) return 'pending-button-not-found';
  hit.click();
  await new Promise((r) => setTimeout(r, 1200));
  return 'pending-opened';
})()`;

const RAF_CLICK = `(async () => {
  const btns = [...document.querySelectorAll('.filmstrip-item')];
  const hit = btns.find((b) =>
    /第 2 张：DSCF1039.RAF/.test(b.getAttribute('aria-label') || ''),
  );
  if (!hit) return 'raf-thumbnail-not-found';
  hit.click();
  await new Promise((r) => setTimeout(r, 1200));
  return 'raf-selected';
})()`;

async function capture(cdp, options) {
  const { name, state, url, width, height, scale, clicks } = options;
  deadlineCheck();
  cdp.loaded = false;
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: scale,
    mobile: false,
  });
  await cdp.send('Page.navigate', { url });
  try {
    await waitFor(
      () =>
        evaluate(
          cdp,
          `!!document.querySelector('.app-shell, .boot-state')`,
        ).then(
          (v) => v === true,
          () => false,
        ),
      15000,
      `app shell for ${name}-${state}`,
    );
  } catch (error) {
    const diag = await evaluate(
      cdp,
      `JSON.stringify({href: location.href, ready: document.readyState, shell: !!document.querySelector('.app-shell'), boot: !!document.querySelector('.boot-state'), text: (document.body?.innerText || '').slice(0, 300)})`,
    ).catch((e) => `diag-error: ${e.message}`);
    log(`DIAG ${name}-${state}: ${diag}`);
    throw error;
  }
  await sleep(1600);
  const clickResults = [];
  for (const click of clicks ?? []) {
    clickResults.push(await evaluate(cdp, click.expression));
    await sleep(900);
  }
  const checks = {};
  const violations = [];
  const expression = STATE_CHECK_EXPRESSIONS[state];
  if (expression) {
    const values = await evaluate(cdp, expression);
    checks.values = values;
    const expected = STATE_EXPECTATIONS[state];
    for (const [key, want] of Object.entries(expected)) {
      const got = values[key];
      if (got !== want) {
        violations.push(`${key}: want ${want}, got ${JSON.stringify(got)}`);
      }
    }
  }
  const generic = await evaluate(cdp, GENERIC_CHECKS);
  checks.generic = generic;
  if (generic.docOverflowX !== false) {
    violations.push('document horizontal overflow');
  }
  if (generic.clippedButtons.length > 0) {
    violations.push(`clipped buttons: ${generic.clippedButtons.join(' | ')}`);
  }
  const shot = await cdp.send('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  const file = join(OUT_DIR, `${name}-${state}.png`);
  writeFileSync(file, Buffer.from(shot.data, 'base64'));
  report.screenshots.push({
    file,
    state,
    width,
    height,
    scale,
    clicks: clickResults,
    checks,
    violations,
  });
  log(
    `captured ${name}-${state}${
      violations.length > 0 ? ` VIOLATIONS: ${violations.join('; ')}` : ''
    }`,
  );
}

const MEASURE = `(() => {
  const rect = (el) => {
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return {
      scrollWidth: el.scrollWidth,
      clientWidth: el.clientWidth,
      width: Math.round(r.width),
      height: Math.round(r.height),
    };
  };
  const filmstrip = document.querySelector('.filmstrip');
  const viewport = document.querySelector('.filmstrip-viewport');
  const track = document.querySelector('.filmstrip-track');
  return {
    viewport: rect(viewport),
    filmstrip: rect(filmstrip),
    track: rect(track),
    inner: { width: innerWidth, height: innerHeight },
    dpr: devicePixelRatio,
    documentHOverflow:
      document.documentElement.scrollWidth > document.documentElement.clientWidth,
  };
})()`;

const GENERIC_CHECKS = `(() => {
  const vw = innerWidth;
  const vh = innerHeight;
  const docOverflowX =
    document.documentElement.scrollWidth > document.documentElement.clientWidth;
  const visible = [...document.querySelectorAll('button')].filter((b) => {
    if (b.classList.contains('filmstrip-item')) return false;
    const r = b.getBoundingClientRect();
    const s = getComputedStyle(b);
    return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 0;
  });
  const clipped = visible
    .filter((b) => {
      const r = b.getBoundingClientRect();
      return (
        r.left < -0.5 || r.top < -0.5 || r.right > vw + 0.5 || r.bottom > vh + 0.5
      );
    })
    .map((b) =>
      (b.getAttribute('aria-label') || b.textContent || '').trim().slice(0, 40),
    );
  return { docOverflowX, clippedButtons: clipped };
})()`;

const LIBRARY_CHECKS = `(() => {
  const rect = (el) => {
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
  };
  const area = (el) => {
    const r = rect(el);
    return r ? r.w * r.h : 0;
  };
  const canvas = document.querySelector('.photo-canvas');
  const inspector = document.querySelector('.photo-inspector');
  const filmstrip = document.querySelector('.filmstrip');
  const workspace = document.querySelector('.photo-workspace');
  const actions = document.querySelector('.library-actions');
  const areas = {
    canvas: area(canvas),
    inspector: area(inspector),
    filmstrip: area(filmstrip),
  };
  const canvasLargest =
    areas.canvas > areas.inspector && areas.canvas > areas.filmstrip;
  const wr = workspace?.getBoundingClientRect();
  const ir = inspector?.getBoundingClientRect();
  const ar = actions?.getBoundingClientRect();
  const pinnedBottom =
    wr && ir && ar
      ? Math.abs(ir.bottom - wr.bottom) <= 2 &&
        Math.abs(ar.bottom - ir.bottom) <= 2
      : false;
  const absPath = /[A-Za-z]:\\\\/.test(document.body.innerText);
  return { canvasLargest, pinnedBottom, absPath, areas };
})()`;

const PENDING_CHECKS = `(() => {
  const bodyText = document.body.innerText;
  const buttons = [...document.querySelectorAll('button')].map((b) =>
    ((b.getAttribute('aria-label') || '') + ' ' + (b.textContent || '')).trim(),
  );
  const hasExternal =
    buttons.some((t) => /Photoshop|文件位置/.test(t)) ||
    /Photoshop|文件位置/.test(bodyText);
  const hasCheckbox =
    !!document.querySelector('input[type="checkbox"]') ||
    /只执行选中项/.test(bodyText);
  const absPath = /[A-Za-z]:\\\\/.test(bodyText);
  const area = (el) => {
    if (!el) return 0;
    const r = el.getBoundingClientRect();
    return r.width * r.height;
  };
  const canvas = document.querySelector('.photo-canvas');
  const inspector = document.querySelector('.photo-inspector');
  const filmstrip = document.querySelector('.filmstrip');
  const workspace = document.querySelector('.photo-workspace');
  const canvasLargest =
    area(canvas) > area(inspector) && area(canvas) > area(filmstrip);
  const actions = document.querySelector('.pending-actions');
  const wr = workspace?.getBoundingClientRect();
  const ir = inspector?.getBoundingClientRect();
  const ar = actions?.getBoundingClientRect();
  const pinnedBottom =
    wr && ir && ar
      ? Math.abs(ir.bottom - wr.bottom) <= 2 &&
        Math.abs(ar.bottom - ir.bottom) <= 2
      : false;
  const executeDisabled =
    document.querySelector('.execute-delete-button')?.disabled ?? null;
  const relativeOnly = /2026\\\\07\\\\30/.test(bodyText);
  const problemVisible = /需要重新确认|需要处理|NeedsAttention/.test(bodyText);
  return {
    hasExternal,
    hasCheckbox,
    absPath,
    canvasLargest,
    pinnedBottom,
    executeDisabled,
    relativeOnly,
    problemVisible,
  };
})()`;

const EMPTY_CHECKS = `(() => {
  const buttons = [...document.querySelectorAll('button')].map((b) =>
    (b.textContent || '').trim(),
  );
  const hasPendingTab = buttons.some((t) => /待删除\\s*\\d+/.test(t));
  const zeroText = /待删除\\s*0/.test(document.body.innerText);
  const emptyMessage = /图库中还没有照片/.test(document.body.innerText);
  return { hasPendingTab, zeroText, emptyMessage };
})()`;

const SETTINGS_CHECKS = `(() => {
  const bodyText = document.body.innerText;
  const sidebar = document.querySelector('.tool-sidebar');
  const groups = sidebar
    ? [...sidebar.querySelectorAll('button')].map((b) =>
        (b.textContent || '').trim(),
      )
    : [];
  const four = ['照片文件夹', '预览缓存', 'Photoshop', '操作记录'].every((g) =>
    groups.some((t) => t.includes(g)),
  );
  const manualCleanup = /手动清理|清理缓存/.test(bodyText);
  const toolId = /20000000-0000|ToolId|内部 ID/.test(bodyText);
  const longPath = !!document.querySelector('.long-path');
  const tabCount = document.querySelectorAll(
    'nav[aria-label="照片视图"] .photo-tab',
  ).length;
  const cacheMetrics = !!document.querySelector('[aria-label="缓存空间摘要"]');
  const capacitySelect = !!document.querySelector(
    '[aria-label="预览缓存容量上限"]',
  );
  const psVerified = /Photoshop.exe/.test(bodyText);
  return {
    four,
    manualCleanup,
    toolId,
    longPath,
    tabCount,
    cacheMetrics,
    capacitySelect,
    psVerified,
  };
})()`;

const LONG_CHECKS = `(() => {
  const bodyText = document.body.innerText;
  const longVisible = /一段非常非常长|一段很长的文件名/.test(bodyText);
  const h2 = document.querySelector('.photo-identity h2');
  const wraps =
    h2 !== null && h2.clientWidth > 0 && h2.scrollWidth <= h2.clientWidth + 1;
  const internal =
    /RootId|LibraryId|PhotoId|AssetId|JobId|AttemptId|Volume GUID|文件 ID|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i.test(
      bodyText,
    );
  return { longVisible, wraps, internal };
})()`;

const UNBOUND_CHECKS = `(() => {
  const bodyText = document.body.innerText;
  const back = [...document.querySelectorAll('button')].some((b) =>
    /返回图库/.test(b.textContent || ''),
  );
  return {
    gate: /选择照片文件夹/.test(bodyText),
    back,
    hasCanvas: !!document.querySelector('.photo-canvas'),
  };
})()`;

const PS_UNCONFIGURED_CHECKS = `(() => {
  const buttons = [...document.querySelectorAll('button')];
  const ps = buttons.find((b) =>
    /Photoshop/.test(
      (b.getAttribute('aria-label') || '') + (b.textContent || ''),
    ),
  );
  return {
    psDisabled: ps ? ps.disabled : null,
    hint: /Photoshop 需要重新选择/.test(document.body.innerText),
  };
})()`;

const STATE_CHECK_EXPRESSIONS = {
  library: LIBRARY_CHECKS,
  long: LONG_CHECKS,
  empty: EMPTY_CHECKS,
  unbound: UNBOUND_CHECKS,
  settings: SETTINGS_CHECKS,
  'settings-cache': SETTINGS_CHECKS,
  'settings-photoshop': SETTINGS_CHECKS,
  pending: PENDING_CHECKS,
  problem: PENDING_CHECKS,
  'photoshop-unconfigured': PS_UNCONFIGURED_CHECKS,
  'photoshop-invalid': PS_UNCONFIGURED_CHECKS,
};

const STATE_EXPECTATIONS = {
  library: { canvasLargest: true, pinnedBottom: true, absPath: false },
  long: { longVisible: true, wraps: true, internal: false },
  empty: { hasPendingTab: false, zeroText: false, emptyMessage: true },
  unbound: { gate: true, back: false, hasCanvas: false },
  settings: {
    four: true,
    manualCleanup: false,
    toolId: false,
    tabCount: 1,
  },
  'settings-cache': {
    four: true,
    manualCleanup: false,
    toolId: false,
    tabCount: 1,
    cacheMetrics: true,
    capacitySelect: true,
  },
  'settings-photoshop': {
    four: true,
    manualCleanup: false,
    toolId: false,
    tabCount: 1,
    psVerified: true,
  },
  pending: {
    hasExternal: false,
    hasCheckbox: false,
    absPath: false,
    canvasLargest: true,
    pinnedBottom: true,
    relativeOnly: true,
  },
  problem: {
    hasExternal: false,
    hasCheckbox: false,
    absPath: false,
    canvasLargest: true,
    pinnedBottom: true,
    relativeOnly: true,
    executeDisabled: true,
    problemVisible: true,
  },
  'photoshop-unconfigured': { psDisabled: false },
  'photoshop-invalid': { psDisabled: true, hint: true },
};

async function measure(cdp, width, height, scale) {
  cdp.loaded = false;
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: scale,
    mobile: false,
  });
  await cdp.send('Page.navigate', { url: `${DEV_URL}/?scenario=library` });
  await waitFor(
    () =>
      evaluate(cdp, `!!document.querySelector('.app-shell')`).then(
        (v) => v === true,
        () => false,
      ),
    15000,
    `filmstrip at ${width}x${height}`,
  );
  await sleep(1600);
  const value = await evaluate(cdp, MEASURE);
  const ok =
    value.viewport !== null &&
    value.viewport.scrollWidth <= value.viewport.clientWidth &&
    value.documentHOverflow === false;
  report.measurements.push({
    width,
    height,
    scale,
    ...value,
    pass: ok,
  });
  log(
    `measured ${width}x${height} dpr=${scale} filmstrip scrollWidth=${value.viewport?.scrollWidth} clientWidth=${value.viewport?.clientWidth} => ${ok ? 'PASS' : 'FAIL'}`,
  );
  return ok;
}

let viteProc = null;
let edgeProc = null;
let cdp = null;
let edgeProfile = null;

async function cleanup(exitCode) {
  if (cdp) {
    try {
      await cdp.send('Browser.close');
    } catch {
      /* noop */
    }
    cdp.close();
  }
  if (edgeProc && !edgeProc.killed) {
    spawnSync('taskkill', ['/pid', String(edgeProc.pid), '/T', '/F'], {
      stdio: 'ignore',
    });
    edgeProc.kill();
  }
  if (viteProc && !viteProc.killed) {
    spawnSync('taskkill', ['/pid', String(viteProc.pid), '/T', '/F'], {
      stdio: 'ignore',
    });
    viteProc.kill();
  }
  if (edgeProfile) {
    try {
      rmSync(edgeProfile, { recursive: true, force: true });
    } catch {
      /* noop */
    }
  }
  process.exit(exitCode);
}

async function main() {
  mkdirSync(OUT_DIR, { recursive: true });
  log(`starting vite on port 1420 (cwd=${ROOT})`);
  viteProc = spawn(
    process.execPath,
    [
      'node_modules/vite/bin/vite.js',
      'preview',
      '--host',
      '127.0.0.1',
      '--port',
      '1420',
      '--strictPort',
    ],
    {
      cwd: ROOT,
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  );
  let viteLogs = '';
  viteProc.stdout.on('data', (chunk) => (viteLogs += chunk));
  viteProc.stderr.on('data', (chunk) => (viteLogs += chunk));
  viteProc.on('exit', (code) => log(`vite exited with code ${code}`));

  await waitFor(
    () => httpReady(`${DEV_URL}/`),
    30000,
    'vite to serve /',
  );
  log('vite is serving');

  edgeProfile = join(tmpdir(), `ui-check-edge-${process.pid}-${Date.now()}`);
  log(`starting headless edge (debug port ${DEBUG_PORT})`);
  edgeProc = spawn(
    EDGE,
    [
      '--headless=new',
      '--no-sandbox',
      '--disable-gpu',
      '--disable-software-rasterizer',
      '--disable-gpu-compositing',
      '--disable-features=VizDisplayCompositor',
      '--no-first-run',
      '--disable-extensions',
      '--disable-background-networking',
      `--remote-debugging-port=${DEBUG_PORT}`,
      `--user-data-dir=${edgeProfile}`,
      'about:blank',
    ],
    { stdio: 'ignore' },
  );
  await waitFor(
    async () => {
      try {
        await httpGetJson(`http://127.0.0.1:${DEBUG_PORT}/json/version`);
        return true;
      } catch {
        return false;
      }
    },
    30000,
    'edge devtools',
  );
  const targets = await httpGetJson(`http://127.0.0.1:${DEBUG_PORT}/json/list`);
  const page = targets.find((target) => target.type === 'page');
  if (!page) throw new Error('no page target');
  cdp = new Cdp(page.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');
  log('cdp connected');

  const sizes = [
    { name: '1440x900-100', width: 1440, height: 900, scale: 1 },
    { name: '1280x720-100', width: 1280, height: 720, scale: 1 },
    { name: '1440x900-150', width: 1440, height: 900, scale: 1.5 },
    { name: '1100x700-100', width: 1100, height: 700, scale: 1 },
  ];

  for (const size of sizes) {
    const base = `${DEV_URL}/?scenario=`;
    await capture(cdp, {
      ...size,
      state: 'library',
      url: `${base}library`,
    });
    await capture(cdp, {
      ...size,
      state: 'settings',
      url: `${base}library`,
      clicks: [{ expression: SETTINGS_CLICK }],
    });
    await capture(cdp, {
      ...size,
      state: 'settings-cache',
      url: `${base}library`,
      clicks: [
        { expression: SETTINGS_CLICK },
        { expression: sidebarClick('\u9884\u89c8\u7f13\u5b58') },
      ],
    });
    await capture(cdp, {
      ...size,
      state: 'settings-photoshop',
      url: `${base}library`,
      clicks: [
        { expression: SETTINGS_CLICK },
        { expression: sidebarClick('Photoshop') },
      ],
    });
    await capture(cdp, {
      ...size,
      state: 'pending',
      url: `${base}pending`,
      clicks: [{ expression: PENDING_CLICK }],
    });
    await capture(cdp, {
      ...size,
      state: 'problem',
      url: `${base}problem`,
      clicks: [{ expression: PENDING_CLICK }],
    });
    await capture(cdp, {
      ...size,
      state: 'long',
      url: `${base}long`,
    });
    await capture(cdp, {
      ...size,
      state: 'empty',
      url: `${base}empty`,
    });
    await capture(cdp, {
      ...size,
      state: 'unbound',
      url: `${base}unbound`,
    });
    await capture(cdp, {
      ...size,
      state: 'photoshop-unconfigured',
      url: `${base}photoshop-unconfigured`,
      clicks: [{ expression: RAF_CLICK }],
    });
    await capture(cdp, {
      ...size,
      state: 'photoshop-invalid',
      url: `${base}photoshop-invalid`,
      clicks: [{ expression: RAF_CLICK }],
    });
  }

  const measurements = [];
  for (const size of sizes) {
    measurements.push(
      await measure(cdp, size.width, size.height, size.scale),
    );
  }
  const screenshotViolations = report.screenshots.filter(
    (s) => s.violations.length > 0,
  );
  const allPass =
    measurements.every(Boolean) && screenshotViolations.length === 0;
  report.pass = allPass && report.failures.length === 0;
  report.screenshotViolationCount = screenshotViolations.length;
  writeFileSync(
    join(OUT_DIR, 'report.json'),
    JSON.stringify(report, null, 2),
  );
  log(`wrote ${report.screenshots.length} screenshots and report.json`);
  log(`filmstrip measurements: ${allPass ? 'ALL PASS' : 'FAILURES'}`);
  for (const violation of screenshotViolations) {
    log(
      `VIOLATION ${violation.file}: ${violation.violations.join('; ')}`,
    );
  }
  await cleanup(allPass ? 0 : 1);
}

const watchdog = setInterval(() => {
  if (Date.now() - START_MS > DEADLINE_MS) {
    log('watchdog: deadline exceeded, cleaning up');
    void cleanup(3);
  }
}, 5000);

main().catch((error) => {
  report.failures.push(String(error?.stack ?? error));
  try {
    writeFileSync(
      join(OUT_DIR, 'report.json'),
      JSON.stringify(report, null, 2),
    );
  } catch {
    /* noop */
  }
  log(`FAILED: ${error?.message ?? error}`);
  clearInterval(watchdog);
  void cleanup(2);
});
