import { buildPac, isProtectedUrl } from './pac.mjs';

const browser = /Edg\//.test(navigator.userAgent) ? 'edge' : 'chrome';
const hostName = 'com.claudeguard.bridge';
let nativePort = null, reconnectTimer = null, heartbeatTimer = null;
let retryDelay = 1000, lastMessageAt = 0, snapshot = null, serial = Promise.resolve();
let confirmed = false, proxyReason = '正在连接桌面守卫', bridgeReason = '';
let queued = false, lastView = '', lastFailure = '';
const getProxy = () => new Promise((resolve, reject) => chrome.proxy.settings.get({ incognito: false }, result => {
  if (chrome.runtime.lastError) reject(new Error(chrome.runtime.lastError.message)); else resolve(result);
}));
const setProxy = value => new Promise((resolve, reject) => chrome.proxy.settings.set({ value, scope: 'regular' }, () => {
  if (chrome.runtime.lastError) reject(new Error(chrome.runtime.lastError.message)); else resolve();
}));
const clearProxy = () => new Promise((resolve, reject) => chrome.proxy.settings.clear({ scope: 'regular' }, () => {
  if (chrome.runtime.lastError) reject(new Error(chrome.runtime.lastError.message)); else resolve();
}));

async function emergencyBlock(enabled) {
  const current = await chrome.declarativeNetRequest.getEnabledRulesets();
  if (current.includes('bootstrap_block') === enabled) return;
  await chrome.declarativeNetRequest.updateEnabledRulesets(enabled
    ? { enableRulesetIds: ['bootstrap_block'] } : { disableRulesetIds: ['bootstrap_block'] });
}

function connected() { return Boolean(nativePort && snapshot && Date.now() - lastMessageAt < 5000); }

function view() {
  const online = connected();
  const state = snapshot?.state || {};
  const status = !online || !confirmed ? 'blocked' : state.status || 'blocked';
  return {
    browser, connected: online, protected: confirmed && online, status,
    reason: !online ? bridgeReason || '桌面守卫未连接；Claude 访问保持阻断' : !confirmed ? proxyReason : state.reason || '等待守卫检查',
    node: state.node || '', country: state.country || '', verifiedAt: state.verifiedAt || null,
    proxyPort: state.proxyPort || 17890,
    updatedAt: lastMessageAt
  };
}

async function publish() {
  const current = view(), encoded = JSON.stringify(current);
  if (encoded === lastView) return;
  lastView = encoded;
  const labels = { allowed: 'US', checking: '…', blocked: '停' };
  const colors = { allowed: '#22c985', checking: '#e6ad3d', blocked: '#ed6575' };
  await Promise.all([
    chrome.storage.session.set({ view: current }),
    chrome.action.setBadgeText({ text: labels[current.status] || '停' }),
    chrome.action.setBadgeBackgroundColor({ color: colors[current.status] || colors.blocked }),
    chrome.action.setTitle({ title: 'Claude 网络守卫 · ' + current.reason })
  ]);
}

async function synchronize() {
  if (!connected()) {
    confirmed = false;
    await emergencyBlock(true);
    await publish();
    return;
  }
  const captured = snapshot;
  let pac;
  try { pac = buildPac(captured.systemProxy, captured.state?.proxyPort || 17890); }
  catch (error) {
    confirmed = false; proxyReason = error.message;
    await emergencyBlock(true);
    const current = await getProxy();
    // Restore the actual system configuration for unrelated websites when it cannot be represented safely.
    if (current.levelOfControl === 'controlled_by_this_extension') await clearProxy();
    await publish();
    return;
  }
  const before = await getProxy();
  if (!['controllable_by_this_extension', 'controlled_by_this_extension'].includes(before.levelOfControl)) {
    confirmed = false;
    proxyReason = before.levelOfControl === 'controlled_by_other_extensions' ? '代理由其他扩展接管；请停用冲突的代理扩展' : '浏览器企业策略禁止代理设置；保护未生效';
    await emergencyBlock(true); await publish(); return;
  }
  if (before.levelOfControl !== 'controlled_by_this_extension' || before.value?.mode !== 'pac_script' ||
      before.value?.pacScript?.data !== pac || before.value?.pacScript?.mandatory !== true) {
    confirmed = false;
    await emergencyBlock(true);
    await setProxy({ mode: 'pac_script', pacScript: { data: pac, mandatory: true } });
  }
  const after = await getProxy();
  confirmed = after.levelOfControl === 'controlled_by_this_extension' && after.value?.mode === 'pac_script' &&
    after.value?.pacScript?.data === pac && after.value?.pacScript?.mandatory === true;
  proxyReason = confirmed ? '' : '浏览器没有确认守卫代理规则；访问保持阻断';
  await emergencyBlock(!confirmed || !connected());
  await publish();
}

function sync() {
  if (queued) return serial;
  queued = true;
  serial = serial.then(async () => {
    queued = false;
    try { await synchronize(); }
    catch (error) {
      confirmed = false; proxyReason = '无法应用浏览器保护：' + error.message;
      try { await emergencyBlock(true); } catch { /* A mandatory persistent PAC remains if already installed. */ }
      await publish();
    }
  }).catch(() => { queued = false; });
  return serial;
}

function post(message) {
  if (!nativePort) return false;
  try { nativePort.postMessage(message); return true; } catch { return false; }
}

function startBridge() {
  if (nativePort) return;
  clearTimeout(reconnectTimer); reconnectTimer = null;
  let port;
  try { port = chrome.runtime.connectNative(hostName); nativePort = port; }
  catch (error) { bridgeReason = '无法启动本机桥接：' + error.message; scheduleReconnect(); sync(); return; }
  port.onMessage.addListener(message => {
    if (port !== nativePort) return;
    if (message?.type === 'status' && message.state && message.systemProxy) {
      snapshot = message; lastMessageAt = Date.now(); retryDelay = 1000; bridgeReason = '';
      const synchronized = sync();
      if (message.command === 'openClaude') {
        synchronized.then(() => {
          if (nativePort === port && connected() && confirmed && view().status === 'allowed')
            return chrome.tabs.create({ url: 'https://claude.ai/' });
        }).catch(() => {});
      }
    } else if (message?.type === 'error') {
      bridgeReason = message.reason || message.message || '桌面守卫通信失败';
      lastMessageAt = 0; sync();
    }
  });
  port.onDisconnect.addListener(() => {
    const nativeError = chrome.runtime.lastError?.message;
    if (port !== nativePort) return;
    nativePort = null; lastMessageAt = 0; confirmed = false;
    bridgeReason = nativeError ? '本机桥接未连接，请启动或修复安装 Claude 网络守卫' : '桌面守卫已断开；Claude 访问保持阻断';
    sync(); scheduleReconnect();
  });
  post({ action: 'browserStatus', browser, connected: true, protected: false, reason: '正在确认守卫代理' });
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  reconnectTimer = setTimeout(() => { reconnectTimer = null; startBridge(); }, retryDelay);
  retryDelay = Math.min(retryDelay * 2, 15000);
}

function heartbeat() {
  const current = view();
  post({ action: 'browserStatus', browser, connected: true, protected: current.protected, reason: current.reason });
  if (!connected()) sync();
  if (!nativePort && !reconnectTimer) startBridge();
}

chrome.proxy.settings.onChange.addListener(() => { confirmed = false; sync(); });
chrome.proxy.onProxyError.addListener(details => {
  if (details.fatal) { confirmed = false; proxyReason = '代理脚本错误，访问保持阻断'; sync(); }
});
chrome.runtime.onInstalled.addListener(details => {
  sync(); startBridge();
  if (details.reason === 'install') chrome.tabs.create({ url: chrome.runtime.getURL('install.html') });
});
chrome.runtime.onStartup.addListener(() => { sync(); startBridge(); });
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'bridge-reconnect') { startBridge(); sync(); } });

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id) return;
  if (message.action === 'getStatus') { sync().then(() => respond(view())); return true; }
  if (['recheck', 'switchPreferred'].includes(message.action)) {
    respond({ ok: post({ action: message.action }), reason: '桌面守卫尚未连接' }); return;
  }
  if (message.action === 'openClaude') {
    const url = isProtectedUrl(message.url) ? message.url : 'https://claude.ai/';
    chrome.tabs.create({ url }); respond({ ok: true }); return;
  }
});

chrome.webNavigation.onErrorOccurred.addListener(details => {
  if (details.frameId !== 0 || !isProtectedUrl(details.url) || details.error === 'net::ERR_ABORTED') return;
  const key = details.tabId + ':' + details.url;
  if (lastFailure === key) return;
  lastFailure = key;
  setTimeout(() => { if (lastFailure === key) lastFailure = ''; }, 2000);
  const blocked = chrome.runtime.getURL('blocked.html') + '#url=' + encodeURIComponent(details.url);
  chrome.tabs.update(details.tabId, { url: blocked }).catch(() => {});
});

// Chromium extension service workers reject top-level await; initialize asynchronously.
chrome.alarms.create('bridge-reconnect', { periodInMinutes: 0.5 }).catch(() => {});
heartbeatTimer = setInterval(heartbeat, 1000);
sync().then(startBridge);
