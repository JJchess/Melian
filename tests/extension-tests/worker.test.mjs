import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';

function event() { const listeners = []; return { addListener: fn => listeners.push(fn), fire: (...args) => listeners.forEach(fn => fn(...args)) }; }
async function flush() { for (let n = 0; n < 25; n++) await new Promise(resolve => setImmediate(resolve)); }
async function worker(existingProxy = { levelOfControl: 'controllable_by_this_extension', value: { mode: 'system' } }) {
  const events = [], timers = [], openedTabs = [];
  let proxy = existingProxy, rules = ['bootstrap_block'], stored = {}, native;
  const settingsChanged = event(), messages = event();
  const chrome = {
    proxy: { settings: {
      get: (_, cb) => cb(proxy),
      set: ({ value }, cb) => { events.push('setProxy'); proxy = { levelOfControl: 'controlled_by_this_extension', value }; cb(); settingsChanged.fire(proxy); },
      clear: (_, cb) => { events.push('clearProxy'); proxy = { levelOfControl: 'controllable_by_this_extension', value: { mode: 'system' } }; cb(); settingsChanged.fire(proxy); },
      onChange: settingsChanged
    }, onProxyError: event() },
    declarativeNetRequest: {
      getEnabledRulesets: async () => rules,
      updateEnabledRulesets: async change => { rules = change.enableRulesetIds || []; events.push(rules.length ? 'block' : 'unblock'); }
    },
    storage: { session: { set: async value => { stored = value; } } },
    action: { setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {}, setTitle: async () => {} },
    runtime: {
      id: 'extension', lastError: null, getURL: value => 'chrome-extension://extension/' + value,
      connectNative: () => { native = { onMessage: event(), onDisconnect: event(), sent: [], postMessage: value => native.sent.push(value) }; return native; },
      onInstalled: event(), onStartup: event(), onMessage: messages
    },
    alarms: { create: async () => {}, onAlarm: event() },
    tabs: { create: async value => { openedTabs.push(value); }, update: async () => {} }, webNavigation: { onErrorOccurred: event() }
  };
  const context = vm.createContext({ chrome, navigator: { userAgent: 'Chrome/140 Edg/140' }, URL, console,
    setTimeout: fn => { timers.push(fn); return timers.length; }, clearTimeout: () => {}, setInterval: fn => { timers.push(fn); return timers.length; } });
  const pac = new vm.SourceTextModule(await readFile(new URL('../../extension/pac.mjs', import.meta.url), 'utf8'), { context });
  const background = new vm.SourceTextModule(await readFile(new URL('../../extension/background.mjs', import.meta.url), 'utf8'), { context });
  await background.link(() => pac); await background.evaluate(); await flush();
  return { events, timers, chrome, openedTabs, get proxy() { return proxy; }, get rules() { return rules; }, get view() { return stored.view; },
    get native() { return native; },
    status: async overrides => { native.onMessage.fire({ type: 'status', command: overrides?.command, state: { status: 'allowed', reason: '美国出口', node: 'US node', country: 'US', proxyPort: 17890, ...overrides?.state }, systemProxy: { supported: true, enabled: true, server: '127.0.0.1:7897', bypass: '', ...overrides?.systemProxy } }); await flush(); },
    conflict: async () => { proxy = { levelOfControl: 'controlled_by_other_extensions', value: { mode: 'direct' } }; settingsChanged.fire(proxy); await flush(); }
  };
}

test('bootstrap blocks Claude without replacing unknown unrelated-system routes', async () => {
  const w = await worker();
  assert.equal(w.proxy.value.mode, 'system'); assert.ok(w.rules.includes('bootstrap_block'));
  assert.equal(w.view.protected, false); assert.equal(w.events.length, 0);
});

test('block is released only after confirmed mandatory persistent PAC, even while guard is checking', async () => {
  const w = await worker(); await w.status({ state: { status: 'checking' } });
  assert.equal(w.proxy.value.mode, 'pac_script'); assert.equal(w.proxy.value.pacScript.mandatory, true);
  assert.equal(w.view.status, 'checking'); assert.equal(w.view.protected, true); assert.equal(w.view.browser, 'edge');
  assert.deepEqual(w.events.slice(0, 2), ['setProxy', 'unblock']);
});

test('desktop shutdown keeps mandatory guard PAC and restores independent blocking', async () => {
  const w = await worker(); await w.status(); const pac = w.proxy.value.pacScript.data;
  w.native.onDisconnect.fire(); await flush();
  assert.equal(w.proxy.value.pacScript.data, pac); assert.ok(w.rules.includes('bootstrap_block'));
  assert.equal(w.view.connected, false); assert.equal(w.view.status, 'blocked');
});

test('proxy ownership conflict blocks domains and never claims protection', async () => {
  const w = await worker(); await w.status(); await w.conflict();
  assert.ok(w.rules.includes('bootstrap_block')); assert.equal(w.view.protected, false);
  assert.match(w.view.reason, /其他扩展/); assert.equal(w.proxy.value.mode, 'direct');
});

test('unsupported system PAC restores system for unrelated websites only after emergency block', async () => {
  const w = await worker(); await w.status(); w.events.length = 0;
  await w.status({ systemProxy: { supported: false, reason: '不支持自动 PAC' } });
  assert.deepEqual(w.events.slice(0, 2), ['block', 'clearProxy']);
  assert.equal(w.view.protected, false); assert.match(w.view.reason, /PAC/);
});

test('service worker restart retains old guard PAC and blocks until fresh native snapshot', async () => {
  const first = await worker(); await first.status();
  const restart = await worker(first.proxy);
  assert.equal(restart.proxy.value.pacScript.data, first.proxy.value.pacScript.data);
  assert.ok(restart.rules.includes('bootstrap_block')); assert.equal(restart.view.protected, false);
  await restart.status(); assert.equal(restart.view.protected, true); assert.deepEqual(restart.rules, []);
});

test('desktop open command opens Claude in this profile only after allowed status and verified PAC', async () => {
  const w = await worker();
  await w.status({ command: 'openClaude' });
  assert.equal(w.openedTabs.length, 1); assert.equal(w.openedTabs[0].url, 'https://claude.ai/');
  assert.equal(w.view.protected, true); assert.equal(w.rules.length, 0);
});

test('desktop open command is rejected for blocked/checking status, proxy conflict and unsupported system proxy', async () => {
  const w = await worker();
  await w.status({ command: 'openClaude', state: { status: 'blocked' } });
  await w.status({ command: 'openClaude', state: { status: 'checking' } });
  await w.status({ command: 'openClaude', systemProxy: { supported: false } });
  await w.conflict(); await w.status({ command: 'openClaude' });
  assert.equal(w.openedTabs.length, 0);
});
