import { isProtectedUrl } from './pac.mjs';
const byId = id => document.getElementById(id);
const target = new URLSearchParams(location.hash.slice(1)).get('url');
function show(state) {
  if (!state) return;
  document.body.dataset.status = state.status;
  byId('status').textContent = ({ allowed: '检查通过 · 已放行', checking: '正在检查 · 暂停连接', blocked: '已阻断连接' })[state.status] || '等待检查';
  byId('reason').textContent = state.reason || '暂无状态';
  byId('node').textContent = state.node || '—';
  byId('country').textContent = state.country === 'US' ? '美国 / US' : state.country || '未验证';
  const date = state.verifiedAt ? new Date(state.verifiedAt) : null;
  byId('verifiedAt').textContent = date && !Number.isNaN(date.getTime()) ? date.toLocaleTimeString('zh-CN') : '—';
  byId('browser').textContent = state.browser === 'edge' ? 'EDGE' : 'CHROME';
}
async function refresh() {
  try { show(await chrome.runtime.sendMessage({ action: 'getStatus' })); }
  catch { byId('reason').textContent = '扩展后台暂未连接，请重新打开此页面'; }
}
for (const action of ['recheck', 'switchPreferred', 'openClaude']) {
  byId(action)?.addEventListener('click', async () => {
    try {
      const result = await chrome.runtime.sendMessage({ action });
      byId('feedback').textContent = result?.ok ? '请求已发送' : result?.reason || '桌面守卫未连接';
      await refresh();
    } catch { byId('feedback').textContent = '无法连接后台，请重新打开扩展'; }
  });
}
byId('retry')?.addEventListener('click', () => { location.href = isProtectedUrl(target) ? target : 'https://claude.ai/'; });
byId('back')?.addEventListener('click', () => history.length > 1 ? history.back() : location.assign('https://www.microsoft.com/'));
chrome.storage.onChanged.addListener((changes, area) => { if (area === 'session' && changes.view) show(changes.view.newValue); });
await refresh();
