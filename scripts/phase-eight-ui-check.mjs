/* global fetch, WebSocket, getComputedStyle, document, Node, innerWidth, innerHeight, devicePixelRatio */
import { createHash } from 'node:crypto';
import { Buffer } from 'node:buffer';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { basename, join, resolve } from 'node:path';
import { setTimeout } from 'node:timers';

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const installedExe = resolve(options['installed-exe'] ?? '');
  const runRoot = resolve(options['run-root'] ?? '');
  if (
    !/^G:\\test\\local-photo-manager-phase8\\fpm-phase8-run-[0-9a-f-]+$/iu.test(
      runRoot,
    )
  ) {
    throw new Error(
      'run-root must be an exact generated G:\\test phase 8 run root',
    );
  }

  const fixture = JSON.parse(
    await runAndCapture(installedExe, [
      '--phase8-self-test',
      'prepare-ui',
      '--run-root',
      runRoot,
    ]),
  );
  const evidenceDirectory = join(runRoot, `ui-candidate-${Date.now()}`);
  await mkdir(evidenceDirectory, { recursive: false });
  const port = await freePort();
  const application = spawn(installedExe, [], {
    env: {
      ...process.env,
      FPM_TEST_ROOT: fixture.testRoot,
      FPM_PHASE8_UI_NO_BACKGROUND_SCAN: '1',
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
    },
    stdio: 'ignore',
    windowsHide: true,
  });

  let cdp;
  try {
    const target = await waitForTarget(port, 45_000);
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.open();
    await cdp.call('Page.enable');
    await cdp.call('Runtime.enable');
    try {
      await waitForExpression(
        cdp,
        "document.querySelectorAll('.filmstrip-item').length > 0",
        20_000,
      );
    } catch (error) {
      const diagnostic = await evaluateValue(
        cdp,
        '({href:location.href,title:document.title,text:document.body?.innerText?.slice(0,2000),classes:[...document.querySelectorAll("[class]")].slice(0,80).map(x=>x.className)})',
      );
      throw new Error(`${error.message}; page=${JSON.stringify(diagnostic)}`, {
        cause: error,
      });
    }

    const matrices = [
      { dpiPercent: 100, deviceScaleFactor: 1, width: 1440, height: 900 },
      { dpiPercent: 125, deviceScaleFactor: 1.25, width: 1280, height: 720 },
      { dpiPercent: 150, deviceScaleFactor: 1.5, width: 1100, height: 700 },
    ];
    const results = [];
    for (const matrix of matrices) {
      await cdp.call('Emulation.setDeviceMetricsOverride', {
        width: matrix.width,
        height: matrix.height,
        deviceScaleFactor: matrix.deviceScaleFactor,
        mobile: false,
      });
      await cdp.call('Runtime.evaluate', {
        expression: 'window.dispatchEvent(new Event("resize"))',
      });
      await delay(300);
      const audit = await evaluateValue(cdp, `(${domAudit.toString()})()`);
      const before = await evaluateValue(
        cdp,
        'document.querySelector(".filmstrip-item[aria-current=true]")?.getAttribute("aria-label") ?? null',
      );
      await cdp.call('Runtime.evaluate', {
        expression:
          'document.querySelector(".filmstrip-item[aria-current=true]")?.focus()',
      });
      await key(cdp, 'ArrowRight');
      const after = await evaluateValue(
        cdp,
        'document.querySelector(".filmstrip-item[aria-current=true]")?.getAttribute("aria-label") ?? null',
      );
      await key(cdp, 'Tab');
      const focus = await evaluateValue(
        cdp,
        '({tag:document.activeElement?.tagName ?? null,label:document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.textContent?.trim().slice(0,80) ?? null})',
      );
      const screenshot = await cdp.call('Page.captureScreenshot', {
        format: 'png',
        fromSurface: true,
        captureBeyondViewport: false,
      });
      const screenshotName = `dpi-${matrix.dpiPercent}-${matrix.width}x${matrix.height}.png`;
      await writeFile(
        join(evidenceDirectory, screenshotName),
        Buffer.from(screenshot.data, 'base64'),
        {
          flag: 'wx',
        },
      );
      results.push({
        ...matrix,
        audit,
        arrowNavigationChanged:
          before !== null && after !== null && before !== after,
        focus,
        screenshot: screenshotName,
        passed:
          !audit.horizontalOverflow &&
          audit.visibleFocusableCount >= 4 &&
          audit.contrastViolations.length === 0 &&
          before !== null &&
          after !== null &&
          before !== after &&
          focus.tag !== null,
      });
    }

    const report = {
      reportVersion: 1,
      candidateExe: installedExe,
      candidateExeSha256: sha256(await readFile(installedExe)),
      candidateExeName: basename(installedExe),
      runRoot,
      fixture,
      chromiumProduct: target.browser ?? null,
      matrices: results,
      passed: results.every((result) => result.passed),
    };
    await writeFile(
      join(evidenceDirectory, 'ui-candidate-report.json'),
      `${JSON.stringify(report, null, 2)}\n`,
      { encoding: 'utf8', flag: 'wx' },
    );
    console.log(JSON.stringify({ ...report, evidenceDirectory }, null, 2));
    if (!report.passed) process.exitCode = 1;
  } finally {
    cdp?.close();
    if (!application.killed) application.kill();
  }
}

function parseArguments(arguments_) {
  const result = {};
  for (let index = 0; index < arguments_.length; index += 2) {
    const key = arguments_[index]?.replace(/^--/u, '');
    const value = arguments_[index + 1];
    if (!key || value === undefined) throw new Error('invalid arguments');
    result[key] = value;
  }
  return result;
}

async function runAndCapture(executable, arguments_) {
  return await new Promise((resolvePromise, reject) => {
    const child = spawn(executable, arguments_, { windowsHide: true });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (value) => (stdout += value));
    child.stderr.on('data', (value) => (stderr += value));
    child.once('error', reject);
    child.once('exit', (code) => {
      if (code === 0) resolvePromise(stdout);
      else reject(new Error(`candidate fixture failed (${code}): ${stderr}`));
    });
  });
}

async function freePort() {
  const server = createServer();
  await new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolvePromise);
  });
  const address = server.address();
  const port = typeof address === 'object' && address ? address.port : 0;
  await new Promise((resolvePromise) => server.close(resolvePromise));
  return port;
}

async function waitForTarget(port, timeout) {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json`);
      const targets = await response.json();
      const pages = targets.filter(
        (target) => target.type === 'page' && target.webSocketDebuggerUrl,
      );
      const page =
        pages.find((target) =>
          /tauri|localhost|fuji/iu.test(`${target.url} ${target.title}`),
        ) ?? pages[0];
      if (page) return page;
    } catch {
      // WebView2 is still starting.
    }
    await delay(200);
  }
  throw new Error('timed out waiting for candidate WebView2 CDP target');
}

async function waitForExpression(cdpClient, expression, timeout) {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    if (await evaluateValue(cdpClient, expression)) return;
    await delay(200);
  }
  throw new Error(`timed out waiting for ${expression}`);
}

async function evaluateValue(cdpClient, expression) {
  const response = await cdpClient.call('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (response.exceptionDetails)
    throw new Error(response.exceptionDetails.text);
  return response.result.value;
}

async function key(cdpClient, keyValue) {
  await cdpClient.call('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: keyValue,
  });
  await cdpClient.call('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: keyValue,
  });
  await delay(100);
}

function delay(milliseconds) {
  return new Promise((resolvePromise) =>
    setTimeout(resolvePromise, milliseconds),
  );
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

class Cdp {
  constructor(url) {
    this.url = url;
    this.socket = null;
    this.nextId = 1;
    this.pending = new Map();
  }

  async open() {
    await new Promise((resolvePromise, reject) => {
      this.socket = new WebSocket(this.url);
      this.socket.addEventListener('open', resolvePromise, { once: true });
      this.socket.addEventListener('error', reject, { once: true });
    });
    this.socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) return;
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(message.error.message));
      else pending.resolve(message.result);
    });
  }

  call(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { resolve: resolvePromise, reject });
      this.socket?.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket?.close();
  }
}

function domAudit() {
  const visible = (element) => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return (
      style.visibility !== 'hidden' &&
      style.display !== 'none' &&
      rect.width > 0 &&
      rect.height > 0
    );
  };
  const rgb = (value) => {
    const match = value.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/u);
    return match ? match.slice(1).map(Number) : null;
  };
  const luminance = (color) => {
    const values = color
      .map((part) => part / 255)
      .map((part) =>
        part <= 0.03928 ? part / 12.92 : ((part + 0.055) / 1.055) ** 2.4,
      );
    return 0.2126 * values[0] + 0.7152 * values[1] + 0.0722 * values[2];
  };
  const background = (element) => {
    let current = element;
    while (current) {
      const value = getComputedStyle(current).backgroundColor;
      if (value && !value.endsWith(', 0)') && value !== 'transparent')
        return rgb(value);
      current = current.parentElement;
    }
    return [255, 255, 255];
  };
  const contrastViolations = [];
  for (const element of document.querySelectorAll('body *')) {
    if (
      !visible(element) ||
      ![...element.childNodes].some(
        (node) => node.nodeType === Node.TEXT_NODE && node.textContent.trim(),
      )
    )
      continue;
    const foreground = rgb(getComputedStyle(element).color);
    const back = background(element);
    if (!foreground || !back) continue;
    const lighter = Math.max(luminance(foreground), luminance(back));
    const darker = Math.min(luminance(foreground), luminance(back));
    const ratio = (lighter + 0.05) / (darker + 0.05);
    const style = getComputedStyle(element);
    const large =
      Number.parseFloat(style.fontSize) >= 24 ||
      (Number.parseFloat(style.fontSize) >= 18.66 &&
        Number.parseInt(style.fontWeight, 10) >= 700);
    const required = large ? 3 : 4.5;
    if (ratio + 0.01 < required)
      contrastViolations.push({
        text: element.textContent.trim().slice(0, 80),
        ratio: Number(ratio.toFixed(2)),
        required,
      });
    if (contrastViolations.length >= 20) break;
  }
  const focusable = [
    ...document.querySelectorAll(
      'button,a[href],input,select,textarea,[tabindex]:not([tabindex="-1"])',
    ),
  ].filter(visible);
  return {
    viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
    horizontalOverflow:
      document.documentElement.scrollWidth >
      document.documentElement.clientWidth + 1,
    visibleFocusableCount: focusable.length,
    contrastViolations,
  };
}

await main();
