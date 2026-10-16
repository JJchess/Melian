// Real-browser acceptance for the native policy route. Requires Node 22+.
// node tests/extension-tests/native-policy-smoke.mjs [edge|chrome|both]
//   [--cli <claude-guard.exe>] [--output <artifact.json>]
//   [--expect-unavailable | --expect-blocked] [--skip-claude-site]
// Policies must already be installed by ClaudeGuard. This test never writes
// policies, installs extensions, touches existing profiles, or stops the guard.
// --expect-unavailable requires the caller to stop the guard separately first.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const root = fileURLToPath(new URL('.', import.meta.url));
const args = process.argv.slice(2);
const selection = args.find(value => ['edge', 'chrome', 'both'].includes(value)) || 'both';
const browsers = selection === 'both' ? ['edge', 'chrome'] : [selection];
const unavailable = args.includes('--expect-unavailable');
const blocked = args.includes('--expect-blocked');
const skipClaude = args.includes('--skip-claude-site');
function option(name, fallback) {
  const index = args.indexOf(name);
  if (index < 0) return fallback;
  assert.ok(args[index + 1] && !args[index + 1].startsWith('--'), `${name} needs a value`);
  return args[index + 1];
}
const cli = option('--cli', path.join(process.env.LOCALAPPDATA || '', 'Programs', 'ClaudeGuard', 'cli', 'claude-guard.exe'));
const output = path.resolve(option('--output', path.join(root, 'artifacts', `native-policy-${selection}${unavailable ? '-unavailable' : ''}.json`)));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const report = { startedAt: new Date().toISOString(), expectUnavailable: unavailable, expectBlocked: blocked, passed: false, browsers: [] };

async function guardState() {
  return await new Promise((resolve, reject) => {
    const child = spawn(cli, ['--status'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', settled = false;
    function finish(error, value) {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      error ? reject(error) : resolve(value);
    }
    const timeout = setTimeout(() => { child.kill(); finish(new Error('Guard status query timed out')); }, 7000);
    child.stdout.on('data', chunk => { stdout += chunk.toString('utf8'); });
    // Do not include CLI stderr: diagnostics can contain local configuration.
    child.stderr.resume();
    child.on('error', error => finish(new Error('Cannot execute guard CLI: ' + error.code)));
    child.on('exit', code => {
      if (code !== 0) { finish(new Error('Guard CLI is unavailable (exit ' + code + ')')); return; }
      try {
        const response = JSON.parse(stdout);
        assert.ok(response.state, 'CLI response has no state');
        finish(null, response.state);
      } catch (error) { finish(error); }
    });
  });
}

async function connect(url) {
  const ws = new WebSocket(url), pending = new Map(), notifications = [];
  let sequence = 0;
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { ws.close(); reject(new Error('CDP connection timed out')); }, 6000);
    ws.onopen = () => { clearTimeout(timeout); resolve(); };
    ws.onerror = () => { clearTimeout(timeout); reject(new Error('CDP connection failed')); };
  });
  ws.onmessage = event => {
    const message = JSON.parse(event.data);
    if (!message.id) {
      notifications.push(message);
      if (notifications.length > 1500) notifications.shift();
      return;
    }
    const task = pending.get(message.id);
    if (!task) return;
    pending.delete(message.id);
    message.error ? task.reject(new Error('CDP: ' + message.error.message)) : task.resolve(message.result);
  };
  ws.onclose = () => {
    for (const task of pending.values()) task.reject(new Error('CDP socket closed'));
    pending.clear();
  };
  return {
    ws, notifications,
    send(method, params = {}) {
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timeout = setTimeout(() => { pending.delete(id); reject(new Error('CDP timeout: ' + method)); }, 30000);
        pending.set(id, {
          resolve: value => { clearTimeout(timeout); resolve(value); },
          reject: error => { clearTimeout(timeout); reject(error); },
        });
        ws.send(JSON.stringify({ id, method, params }));
      });
    },
  };
}

// Chromium's policy-row exposes the structured PolicyUiModel. Walk shadow
// roots because recent Chrome/Edge render the table inside custom elements.
const policyExpression = `(() => {
  const rows = [];
  function visit(root) {
    for (const el of root.querySelectorAll('*')) {
      const p = el.policy || el.policy_;
      if (p?.name === 'ProxySettings' && p.value !== undefined) rows.push({
        name: p.name, value: p.value, error: p.error || '', warning: p.warning || '',
        ignored: !!p.ignored, future: !!p.future, conflicts: p.conflicts || [],
        source: p.source, scope: p.scope, level: p.level, status: p.status || '',
        restartRequired: !!p.restartRequired
      });
      if (el.shadowRoot) visit(el.shadowRoot);
    }
  }
  visit(document);
  // Edge currently uses a React table rather than Chromium's policy-row.
  // Expand the visible row to read the complete JSON; the collapsed cell and
  // its title are truncated. Only the temporary profile's UI is changed.
  if (!rows.length) {
    const button = document.getElementById('ProxySettings-button-id');
    const row = button?.closest('[role="row"]');
    if (row) {
      const cells = [...row.querySelectorAll(':scope > [role="cell"]')];
      const details = row.nextElementSibling;
      if (details?.getAttribute('role') !== 'row') { button.click(); return []; }
      try {
        const value = JSON.parse(details.querySelector('[role="cell"]').textContent);
        const status = cells[4].textContent.trim();
        // Fail on unknown/translated notices, instead of assuming that the
        // presence of the value alone means the policy was applied.
        rows.push({ name: 'ProxySettings', value, status,
          error: /^(OK|确定|確定|正常)$/i.test(status) ? '' : status,
          warning: '', ignored: false, future: false, conflicts: [],
          source: cells[1].textContent.trim(), scope: cells[2].textContent.trim(),
          level: cells[3].textContent.trim(), restartRequired: false });
      } catch { }
    }
  }
  return rows;
})()`;

async function readPolicy(page, guardPort) {
  let policies = [];
  for (let attempt = 0; attempt < 60; attempt++) {
    const result = await page.send('Runtime.evaluate', { expression: policyExpression, returnByValue: true });
    policies = result.result?.value || [];
    if (policies.length) break;
    await sleep(250);
  }
  assert.ok(policies.length, 'ProxySettings policy was not found; native protection is not installed or not visible in this process context');
  const policy = policies[0];
  assert.equal(Boolean(policy.error), false, 'ProxySettings has a policy error');
  assert.equal(policy.ignored, false, 'ProxySettings is ignored');
  assert.equal(policy.future, false, 'ProxySettings is unsupported by this browser version');
  assert.equal(policy.restartRequired, false, 'Fresh browser still reports a policy restart requirement');
  assert.equal(policy.conflicts.length, 0, 'ProxySettings has conflicting policies');
  let value = policy.value;
  if (typeof value === 'string') value = JSON.parse(value);
  assert.equal(value.ProxyMode, 'pac_script');
  assert.equal(value.ProxyPacMandatory, true, 'PAC must be mandatory');
  assert.match(value.ProxyPacUrl, /^data:application\/x-ns-proxy-autoconfig,/, 'PAC must persist in the policy without a running HTTP server');
  const pac = decodeURIComponent(value.ProxyPacUrl.slice(value.ProxyPacUrl.indexOf(',') + 1));
  assert.match(pac, /ClaudeGuard persistent fail-closed browser route/, 'ProxySettings is not the ClaudeGuard route');
  const sandbox = vm.createContext(Object.create(null));
  vm.runInContext(pac, sandbox, { timeout: 1000 });
  const protectedHosts = ['claude.ai', 'api.claude.ai', 'claude.com', 'api.claude.com', 'anthropic.com', 'api.anthropic.com', 'connect.claudeguard.invalid'];
  for (const host of protectedHosts) {
    sandbox.probeHost = host;
    const route = vm.runInContext('FindProxyForURL("https://" + probeHost + "/", probeHost)', sandbox, { timeout: 1000 });
    assert.equal(route, `PROXY 127.0.0.1:${guardPort}`, 'Protected host has an unexpected route: ' + host);
  }
  return {
    name: policy.name, source: policy.source, scope: policy.scope, level: policy.level,
    status: policy.status, warning: Boolean(policy.warning), mandatory: true,
    persistentDataPac: true, pacSha256: createHash('sha256').update(pac).digest('hex'),
    protectedDomainRoutesVerified: true,
  };
}

async function navigate(page, url) {
  page.notifications.length = 0;
  const navigation = await page.send('Page.navigate', { url });
  const deadline = Date.now() + 25000;
  let response, failure;
  while (Date.now() < deadline) {
    const requests = page.notifications.filter(event => event.method === 'Network.requestWillBeSent' &&
      event.params.type === 'Document' && event.params.frameId === navigation.frameId &&
      (!navigation.loaderId || event.params.loaderId === navigation.loaderId));
    const ids = new Set(requests.map(event => event.params.requestId));
    const responses = page.notifications.filter(event => event.method === 'Network.responseReceived' &&
      event.params.type === 'Document' && event.params.frameId === navigation.frameId &&
      (!navigation.loaderId || event.params.loaderId === navigation.loaderId) &&
      /^https?:/.test(event.params.response.url));
    response = responses.at(-1)?.params.response;
    failure = page.notifications.find(event => event.method === 'Network.loadingFailed' && ids.has(event.params.requestId))?.params.errorText;
    if (response || failure || navigation.errorText) break;
    await sleep(100);
  }
  return {
    status: response?.status ?? null,
    remoteAddress: response?.remoteIPAddress || '',
    remotePort: response?.remotePort || null,
    failure: failure || navigation.errorText || (response ? '' : 'No document response within 25 seconds'),
  };
}

async function navigateFresh(browserSocket, base, url) {
  // Chromium can detach a page's CDP session while replacing a navigation
  // with an internal HTTP/proxy error page. Use one owned target per check.
  const created = await browserSocket.send('Target.createTarget', { url: 'about:blank' });
  let page;
  try {
    let target;
    for (let attempt = 0; attempt < 30; attempt++) {
      target = (await (await fetch(base + '/json/list')).json()).find(item => item.id === created.targetId);
      if (target?.webSocketDebuggerUrl) break;
      await sleep(100);
    }
    assert.ok(target?.webSocketDebuggerUrl, 'New test page did not expose a CDP target');
    page = await connect(target.webSocketDebuggerUrl);
    await page.send('Page.enable');
    await page.send('Network.enable');
    await page.send('Network.setCacheDisabled', { cacheDisabled: true });
    return await navigate(page, url);
  } finally {
    page?.ws.close();
    try { await browserSocket.send('Target.closeTarget', { targetId: created.targetId }); } catch { }
  }
}

async function runBrowser(browser, state) {
  const result = { browser, passed: false };
  report.browsers.push(result);
  const executable = browser === 'edge'
    ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe'
    : 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe';
  const profile = await mkdtemp(path.join(root, '.smoke-profile-native-'));
  let browserSocket, page, child, childError;
  try {
    child = spawn(executable, [
      '--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-gpu',
      '--disable-sync', '--disable-features=msEdgeSidebarV2', '--remote-debugging-port=0',
      '--user-data-dir=' + profile, 'about:blank',
    ], { windowsHide: true, stdio: 'ignore' });
    child.on('error', error => { childError = error; });
    result.ownedBrowserPid = child.pid;
    let port;
    for (let attempt = 0; attempt < 80; attempt++) {
      assert.ok(!childError, 'Browser launch failed: ' + childError?.code);
      assert.equal(child.exitCode, null, 'Browser exited before CDP was ready');
      try { port = Number((await readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); } catch { }
      if (port) break;
      await sleep(250);
    }
    assert.ok(port, 'Browser did not start remote debugging');
    const base = 'http://127.0.0.1:' + port;
    const version = await (await fetch(base + '/json/version')).json();
    result.version = version.Browser;
    browserSocket = await connect(version.webSocketDebuggerUrl);
    const target = (await (await fetch(base + '/json/list')).json()).find(item => item.type === 'page');
    assert.ok(target, 'No isolated browser page target');
    page = await connect(target.webSocketDebuggerUrl);
    await page.send('Page.enable');
    await page.send('Network.enable');
    await page.send('Network.setCacheDisabled', { cacheDisabled: true });
    await page.send('Page.navigate', { url: browser + '://policy/' });
    result.policy = await readPolicy(page, state?.proxyPort || 17890);

    result.probe = await navigateFresh(browserSocket, base, 'http://connect.claudeguard.invalid/smoke-invalid-' + randomUUID());
    if (unavailable) {
      assert.match(result.probe.failure, /^net::ERR_PROXY_CONNECTION_FAILED$/, 'Protected probe did not fail at the unavailable guard proxy');
      assert.equal(result.probe.status, null, 'Unexpected HTTP response while guard is unavailable');
      result.protectedHostFailsClosed = true;
    } else {
      assert.equal(result.probe.status, blocked ? 503 : 404, 'Probe did not receive the expected local guard response: ' + JSON.stringify(result.probe));
      assert.equal(result.probe.remoteAddress, '127.0.0.1', 'Probe response did not come from loopback');
      assert.equal(result.probe.remotePort, state.proxyPort, 'Probe response did not come from the guard proxy port');
      result.actualGuardRouteVerified = true;
    }

    result.unrelatedSite = await navigateFresh(browserSocket, base, 'https://example.com/');
    assert.ok(result.unrelatedSite.status >= 200 && result.unrelatedSite.status < 400,
      'Unrelated website did not succeed: ' + JSON.stringify(result.unrelatedSite));
    assert.notEqual(result.unrelatedSite.remotePort, state?.proxyPort || 17890, 'Unrelated website was incorrectly routed through the protected guard port');
    result.unrelatedSiteAvailable = true;

    if (!skipClaude) {
      result.claude = await navigateFresh(browserSocket, base, 'https://claude.ai/');
      if (unavailable) {
        assert.match(result.claude.failure, /^net::ERR_PROXY_CONNECTION_FAILED$/, 'Claude did not fail at the unavailable guard proxy');
      } else if (blocked) {
        assert.match(result.claude.failure, /^net::ERR_TUNNEL_CONNECTION_FAILED$/, 'Claude did not reject the blocked CONNECT tunnel');
        assert.equal(result.claude.status, null, 'Claude received an unexpected response while blocked');
      } else {
        // A remote 403 challenge is an HTTP response and still proves the
        // transport route. This test never claims account/login acceptance.
        assert.ok(result.claude.status, 'Claude returned no response: ' + result.claude.failure);
        assert.equal(result.claude.remoteAddress, '127.0.0.1', 'Claude response bypassed the local proxy');
        assert.equal(result.claude.remotePort, state.proxyPort, 'Claude response used a different proxy port');
      }
    } else result.claude = { skipped: 'Explicit --skip-claude-site' };
    result.passed = true;
  } catch (error) {
    result.error = error.message;
    throw error;
  } finally {
    page?.ws.close();
    if (browserSocket) {
      try { await Promise.race([browserSocket.send('Browser.close'), sleep(2000)]); } catch { }
      browserSocket.ws.close();
    }
    if (child && child.exitCode === null) {
      await Promise.race([new Promise(resolve => child.once('exit', resolve)), sleep(1500)]);
      if (child.exitCode === null) child.kill();
    }
    await sleep(500);
    const resolved = path.resolve(profile);
    assert.equal(path.dirname(resolved).toLowerCase(), path.resolve(root).toLowerCase(), 'Refusing profile removal outside the test directory');
    assert.ok(path.basename(resolved).startsWith('.smoke-profile-native-'), 'Refusing removal of an unowned profile');
    await rm(resolved, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  }
}

try {
  assert.equal(process.platform, 'win32', 'Native browser policy acceptance requires Windows');
  assert.ok(!(unavailable && blocked), 'Choose only one of --expect-unavailable and --expect-blocked');
  let state;
  try { state = await guardState(); }
  catch (error) {
    if (!unavailable) throw error;
    report.guardStatusUnavailable = true;
  }
  if (unavailable) {
    assert.ok(!state, '--expect-unavailable requires the guard to already be stopped; this script never stops it');
  } else {
    report.guardStatus = { allowed: state.allowed, status: state.status, proxyPort: state.proxyPort };
    for (const browser of browsers) {
      const configured = state.browsers?.[browser];
      assert.equal(configured?.configured, true, `${browser}: native browser configuration is not installed`);
      assert.equal(configured.connectionKind, 'native', `${browser}: browser is not configured for the native route`);
    }
  }
  for (const browser of browsers) {
    if (!unavailable) {
      const deadline = Date.now() + 60000;
      do {
        state = await guardState();
        if (state.allowed === !blocked) break;
        await sleep(500);
      } while (Date.now() < deadline);
      assert.equal(state.allowed, !blocked, blocked ? 'Guard did not enter the expected blocked state' : 'Guard did not become ready within 60 seconds');
    }
    await runBrowser(browser, state);
  }
  report.passed = true;
} catch (error) {
  report.error = error.message;
  process.exitCode = 1;
} finally {
  report.finishedAt = new Date().toISOString();
  await mkdir(path.dirname(output), { recursive: true });
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report, null, 2));
  console.log('Artifact: ' + output);
}
