// Optional real-browser check: node tests/extension-tests/browser-smoke.mjs [edge|chrome] [--require-connected] [--require-open-web]
// Uses an isolated disposable profile; never touches an existing browser profile.
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, mkdir, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import assert from 'node:assert/strict';

const root = fileURLToPath(new URL('.', import.meta.url));
const extension = fileURLToPath(new URL('../../extension', import.meta.url));
const browser = process.argv.slice(2).find(arg => arg === 'edge' || arg === 'chrome') || 'edge';
const requireOpenWeb = process.argv.includes('--require-open-web');
const requireConnected = requireOpenWeb || process.argv.includes('--require-connected');
const verifyLifecycle = process.argv.includes('--verify-lifecycle');
const executable = browser === 'edge' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe';
const profile = await mkdtemp(path.join(root, '.smoke-profile-'));
const artifacts = path.join(root, 'artifacts');
await mkdir(artifacts, { recursive: true });
const child = spawn(executable, ['--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-gpu', '--disable-sync',
  '--disable-features=msEdgeSidebarV2', '--enable-unsafe-extension-debugging', '--remote-debugging-port=0', '--user-data-dir=' + profile,
  'about:blank'], { windowsHide: true, stdio: 'ignore' });
let browserSocket;
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function connect(url) {
  const ws = new WebSocket(url), pending = new Map(), notifications = []; let sequence = 0;
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  ws.onmessage = event => {
    const message = JSON.parse(event.data);
    if (!message.id) { notifications.push(message); if (notifications.length > 1000) notifications.shift(); return; }
    const task = pending.get(message.id); if (!task) return; pending.delete(message.id);
    message.error ? task.reject(new Error(JSON.stringify(message.error))) : task.resolve(message.result);
  };
  return { ws, notifications, send(method, params = {}) { return new Promise((resolve, reject) => {
    const id = ++sequence, timeout = setTimeout(() => { pending.delete(id); reject(new Error('CDP timeout: ' + method)); }, 8000);
    pending.set(id, { resolve: value => { clearTimeout(timeout); resolve(value); }, reject: error => { clearTimeout(timeout); reject(error); } });
    ws.send(JSON.stringify({ id, method, params }));
  }); } };
}
async function openThroughDesktop() {
  const cli = path.join(process.env.LOCALAPPDATA, 'Programs', 'ClaudeGuard', 'cli', 'claude-guard.exe');
  return await new Promise((resolve, reject) => {
    const processHandle = spawn(cli, ['--open-web'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '', error = '';
    const timeout = setTimeout(() => { processHandle.kill(); reject(new Error('Installed CLI --open-web timed out')); }, 10000);
    processHandle.stdout.on('data', chunk => { output += chunk.toString('utf8'); });
    processHandle.stderr.on('data', chunk => { error += chunk.toString('utf8'); });
    processHandle.on('error', failure => { clearTimeout(timeout); reject(failure); });
    processHandle.on('exit', code => {
      clearTimeout(timeout);
      if (code !== 0) { reject(new Error('Installed CLI --open-web failed: ' + (error || output).trim())); return; }
      try { const response = JSON.parse(output.trim()); assert.equal(response.ok, true); resolve(response); }
      catch (failure) { reject(failure); }
    });
  });
}
try {
  let port;
  for (let attempt = 0; attempt < 60; attempt++) {
    try { port = Number((await readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); break; } catch { await sleep(250); }
  }
  assert.ok(port, 'Browser did not start remote debugging');
  const version = await (await fetch('http://127.0.0.1:' + port + '/json/version')).json();
  console.log('Browser: ' + version.Browser);
  browserSocket = await connect(version.webSocketDebuggerUrl);
  const id = (await readFile(path.join(extension, 'extension-id.txt'), 'utf8')).trim();
  try { const loaded = await browserSocket.send('Extensions.loadUnpacked', { path: extension }); console.log('Loaded extension: ' + loaded.id); }
  catch (error) { console.log('CDP extension loader: ' + error.message); }
  let target;
  for (let attempt = 0; attempt < 40; attempt++) {
    const targets = await (await fetch('http://127.0.0.1:' + port + '/json/list')).json();
    target = targets.find(item => item.type === 'service_worker' && item.url.includes(id));
    if (target) break;
    await sleep(250);
  }
  if (!target) {
    const created = await browserSocket.send('Target.createTarget', { url: 'chrome://extensions/' });
    await sleep(500);
    const pages = await (await fetch('http://127.0.0.1:' + port + '/json/list')).json();
    const inspect = await connect(pages.find(item => item.id === created.targetId).webSocketDebuggerUrl);
    const info = await inspect.send('Runtime.evaluate', { expression: `new Promise(r=>chrome.developerPrivate.getExtensionsInfo({includeDisabled:true,includeTerminated:true},r))`, awaitPromise: true, returnByValue: true });
    console.log(JSON.stringify(info.result?.value || info, null, 2));
    inspect.ws.close();
  }
  assert.ok(target, 'Browser did not start the extension service worker');
  // Query from an extension page: Edge can expose a worker target before its
  // extension execution context exists. The stored view is still published by
  // the real worker/native bridge, and the heartbeat below must advance.
  const created = await browserSocket.send('Target.createTarget', { url: 'chrome-extension://' + id + '/popup.html' });
  const pages = await (await fetch('http://127.0.0.1:' + port + '/json/list')).json();
  const page = await connect(pages.find(item => item.id === created.targetId).webSocketDebuggerUrl);
  await page.send('Runtime.enable');
  let apiReady = false;
  for (let attempt = 0; attempt < 40; attempt++) {
    const result = await page.send('Runtime.evaluate', { expression: 'typeof chrome !== "undefined" && !!chrome.proxy?.settings', returnByValue: true });
    if (result.result.value === true) { apiReady = true; break; }
    await sleep(250);
  }
  assert.ok(apiReady, 'Extension page did not initialize its Chrome APIs');
  const expression = `(async()=>({proxy:await new Promise(r=>chrome.proxy.settings.get({incognito:false},r)),rules:await chrome.declarativeNetRequest.getEnabledRulesets(),view:(await chrome.storage.session.get('view')).view}))()`;
  async function readState() {
    const result = await page.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    assert.ok(!result.exceptionDetails, JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
  let state = await readState();
  let bridgeHeartbeatVerified = false, desktopOpenVerified = false, desktopOpenSkipped = '';
  if (requireConnected) {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !(state.view?.connected && state.view?.protected &&
      state.proxy.levelOfControl === 'controlled_by_this_extension' && state.proxy.value?.pacScript?.mandatory === true)) {
      await sleep(250); state = await readState();
    }
    assert.equal(state.view?.connected, true, 'Native bridge did not connect within 15 seconds: ' + state.view?.reason);
    assert.equal(state.view?.protected, true, 'Browser did not confirm protection: ' + state.view?.reason);
    assert.equal(state.proxy.levelOfControl, 'controlled_by_this_extension');
    assert.equal(state.proxy.value?.pacScript?.mandatory, true);
    assert.ok(!state.rules.includes('bootstrap_block'), 'Bootstrap block was not released after confirmed PAC');
    const firstHeartbeat = state.view.updatedAt;
    await sleep(2200); state = await readState();
    assert.ok(state.view.connected && state.view.protected && state.view.updatedAt > firstHeartbeat, 'Native bridge heartbeat did not remain active');
    bridgeHeartbeatVerified = true;
    if (state.view.status === 'allowed') {
      await browserSocket.send('Target.setDiscoverTargets', { discover: true });
      browserSocket.notifications.length = 0;
      await openThroughDesktop();
      const openedDeadline = Date.now() + 6000;
      while (Date.now() < openedDeadline) {
        desktopOpenVerified = browserSocket.notifications.some(message =>
          ['Target.targetCreated', 'Target.targetInfoChanged'].includes(message.method) &&
          /^https:\/\/claude\.ai\//.test(message.params?.targetInfo?.url || ''));
        if (desktopOpenVerified) break;
        await sleep(100);
      }
      assert.ok(desktopOpenVerified, 'Desktop --open-web did not open Claude in this protected profile');
    } else {
      desktopOpenSkipped = 'Network is not allowed: ' + state.view.reason;
      assert.ok(!requireOpenWeb, desktopOpenSkipped);
    }
  }
  assert.ok(state.view, 'Service worker did not publish state');
  assert.ok(state.rules.includes('bootstrap_block') || state.proxy.value?.pacScript?.mandatory === true, 'Neither bootstrap block nor mandatory PAC protects browser');
  const lifecycle = {};
  if (verifyLifecycle) {
    assert.equal(state.view.protected, true);
    const pacBefore = state.proxy.value.pacScript.data;
    await page.send('ServiceWorker.enable');
    let versionId;
    for (let i = 0; i < 40 && !versionId; i++) {
      versionId = page.notifications.filter(n => n.method === 'ServiceWorker.workerVersionUpdated')
        .flatMap(n => n.params.versions).find(v => v.scriptURL.includes(id) && v.runningStatus === 'running')?.versionId;
      if (!versionId) await sleep(100);
    }
    assert.ok(versionId, 'No running extension service worker version');
    await page.send('ServiceWorker.stopWorker', { versionId });
    state = await readState();
    assert.equal(state.proxy.value.pacScript.mandatory, true);
    assert.equal(state.proxy.value.pacScript.data, pacBefore);
    lifecycle.workerStopKeepsMandatoryPac = true;
    // A later-installed proxy extension takes ownership in this disposable profile.
    const conflictPath = path.join(profile, 'qa-proxy-conflict');
    await mkdir(conflictPath);
    await writeFile(path.join(conflictPath, 'manifest.json'), JSON.stringify({ manifest_version: 3, name: 'QA proxy conflict', version: '1.0', permissions: ['proxy'] }));
    await writeFile(path.join(conflictPath, 'probe.html'), '<!doctype html><title>QA proxy conflict</title>');
    const other = await browserSocket.send('Extensions.loadUnpacked', { path: conflictPath });
    const otherTarget = await browserSocket.send('Target.createTarget', { url: 'chrome-extension://' + other.id + '/probe.html' });
    await sleep(400);
    const otherPages = await (await fetch('http://127.0.0.1:' + port + '/json/list')).json();
    const otherPage = await connect(otherPages.find(p => p.id === otherTarget.targetId).webSocketDebuggerUrl);
    try {
      const changed = await otherPage.send('Runtime.evaluate', { expression: 'new Promise(r=>chrome.proxy.settings.set({value:{mode:"direct"},scope:"regular"},()=>r(chrome.runtime.lastError?.message||"ok")))', awaitPromise: true, returnByValue: true });
      assert.equal(changed.result.value, 'ok');
      for (let i = 0; i < 60; i++) {
        state = await readState();
        if (!state.view.protected && state.rules.includes('bootstrap_block') && state.proxy.levelOfControl === 'controlled_by_other_extensions') break;
        await sleep(250);
      }
      assert.equal(state.view.protected, false);
      assert.equal(state.proxy.levelOfControl, 'controlled_by_other_extensions');
      assert.ok(state.rules.includes('bootstrap_block'));
      lifecycle.proxyConflictBlocksAndReports = true;
    } finally {
      otherPage.ws.close();
      await browserSocket.send('Extensions.uninstall', { id: other.id });
    }
    for (let i = 0; i < 60; i++) {
      state = await readState();
      if (state.view.protected) break;
      await sleep(250);
    }
    assert.equal(state.view.protected, true);
    lifecycle.removingConflictRestoresProtection = true;
  }
  await page.send('Emulation.setDeviceMetricsOverride', { width: 360, height: 640, deviceScaleFactor: 1, mobile: false });
  const screenshot = await page.send('Page.captureScreenshot', { format: 'png' });
  await writeFile(path.join(artifacts, browser + '-popup.png'), Buffer.from(screenshot.data, 'base64'));
  await writeFile(path.join(artifacts, browser + '-smoke.json'), JSON.stringify({ browser: version.Browser, requireConnected, bridgeHeartbeatVerified, desktopOpenVerified, desktopOpenSkipped, lifecycle, state }, null, 2));
  console.log(JSON.stringify({ browser: version.Browser, status: state.view.status, protected: state.view.protected, reason: state.view.reason, bridgeHeartbeatVerified, desktopOpenVerified, desktopOpenSkipped, lifecycle, screenshot: path.join(artifacts, browser + '-popup.png') }, null, 2));
  page.ws.close();
} finally {
  if (browserSocket) {
    try { await Promise.race([browserSocket.send('Browser.close'), sleep(1000)]); } catch { }
    browserSocket.ws.close();
  }
  if (child.exitCode === null) child.kill();
  await sleep(500);
  const resolved = path.resolve(profile);
  if (!resolved.startsWith(path.resolve(root) + path.sep) || !path.basename(resolved).startsWith('.smoke-profile-')) throw new Error('Refusing to remove profile outside test directory');
  await rm(resolved, { recursive: true, force: true, maxRetries: 3, retryDelay: 500 });
}
