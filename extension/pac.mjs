export const protectedDomains = Object.freeze(['claude.ai', 'claude.com', 'anthropic.com']);

export function isProtectedUrl(value) {
  try {
    const url = new URL(value);
    const host = url.hostname.toLowerCase().replace(/\.$/, '');
    return ['http:', 'https:', 'ws:', 'wss:'].includes(url.protocol) &&
      protectedDomains.some(domain => host === domain || host.endsWith('.' + domain));
  } catch { return false; }
}

function endpoint(raw, defaultKind = 'PROXY') {
  const match = /^(?:(https?|socks4?|socks5):\/\/)?(\[[0-9a-f:]+\]|[a-z0-9._-]+)(?::([0-9]+))?$/i.exec(raw.trim());
  if (!match) throw new Error('系统静态代理地址不受支持');
  const kind = ({ http: 'PROXY', https: 'HTTPS', socks: 'SOCKS', socks4: 'SOCKS', socks5: 'SOCKS5' })[match[1]?.toLowerCase()] || defaultKind;
  const port = Number(match[3] || (kind === 'HTTPS' ? 443 : kind.startsWith('SOCKS') ? 1080 : 80));
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('系统代理端口无效');
  return `${kind} ${match[2]}:${port}`;
}

export function parseSystemProxy(system) {
  if (!system || system.supported !== true) throw new Error(system?.reason || '等待受支持的系统静态代理配置');
  const config = { routes: {}, bypass: [], implicitLoopback: true };
  if (!system.enabled) return config;
  const server = String(system.server || '').trim();
  if (!server) throw new Error('系统代理已启用，但地址为空');
  if (!server.includes('=')) config.routes.all = endpoint(server);
  else {
    for (const item of server.split(';').map(value => value.trim()).filter(Boolean)) {
      const match = /^(http|https|ftp|socks)=(.+)$/i.exec(item);
      if (!match || config.routes[match[1].toLowerCase()]) throw new Error('系统代理映射格式不受支持');
      const scheme = match[1].toLowerCase();
      config.routes[scheme] = endpoint(match[2], scheme === 'socks' ? 'SOCKS' : 'PROXY');
    }
  }
  for (let raw of String(system.bypass || '').split(/[;,]/).map(value => value.trim().toLowerCase()).filter(Boolean)) {
    if (raw === '<-loopback>') { config.implicitLoopback = false; continue; }
    if (raw === '<local>') { config.bypass.push({ local: true }); continue; }
    const cidr = /^(\d{1,3}(?:\.\d{1,3}){3})\/(\d{1,2})$/.exec(raw);
    if (cidr) {
      if (cidr[1].split('.').some(n => Number(n) > 255) || Number(cidr[2]) > 32) throw new Error('系统代理绕过网段无效');
      config.bypass.push({ network: cidr[1], prefix: Number(cidr[2]) }); continue;
    }
    const match = /^(?:(https?|ftp|wss?):\/\/)?(\[[0-9a-f:]+\]|[a-z0-9*?._-]+)(?::([0-9]+))?$/.exec(raw);
    if (!match) throw new Error('系统代理包含不受支持的绕过规则');
    if (match[3] && (Number(match[3]) < 1 || Number(match[3]) > 65535)) throw new Error('系统代理绕过端口无效');
    config.bypass.push({ scheme: match[1] || '', host: match[2].startsWith('.') ? '*' + match[2] : match[2], port: Number(match[3] || 0) });
  }
  return config;
}

// This function is serialized into PAC. Keep it standalone and ES5-compatible.
function route(url, host, config, guardPort, domains) {
  host = host.toLowerCase().replace(/\.$/, '');
  for (var i = 0; i < domains.length; i++) {
    if (host === domains[i] || host.slice(-(domains[i].length + 1)) === '.' + domains[i]) return 'PROXY 127.0.0.1:' + guardPort;
  }
  var match = /^([a-z]+):\/\/[^/]+/i.exec(url);
  var scheme = match ? match[1].toLowerCase() : 'https';
  var authority = match ? match[0].replace(/^[a-z]+:\/\//i, '').replace(/^.*@/, '') : host;
  var portMatch = /:([0-9]+)$/.exec(authority);
  var port = portMatch ? Number(portMatch[1]) : scheme === 'https' || scheme === 'wss' ? 443 : scheme === 'ftp' ? 21 : 80;
  var normalizedHost = host.replace(/^\[|\]$/g, '');
  if (config.implicitLoopback && (normalizedHost === '::1' || /^127\./.test(host) || host === 'localhost' || /\.localhost$/.test(host))) return 'DIRECT';
  function ipNumber(ip) {
    if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(ip)) return null;
    var parts = ip.split('.'), value = 0;
    for (var j = 0; j < 4; j++) { if (+parts[j] > 255) return null; value = value * 256 + Number(parts[j]); }
    return value >>> 0;
  }
  for (var n = 0; n < config.bypass.length; n++) {
    var rule = config.bypass[n];
    if (rule.local && host.indexOf('.') < 0 && host.indexOf(':') < 0) return 'DIRECT';
    if (rule.network) {
      var address = ipNumber(host), network = ipNumber(rule.network), mask = rule.prefix ? (0xffffffff << (32 - rule.prefix)) >>> 0 : 0;
      if (address !== null && (address & mask) === (network & mask)) return 'DIRECT';
    }
    if (rule.host && (!rule.scheme || scheme === rule.scheme) && (!rule.port || port === rule.port)) {
      var pattern = '^' + rule.host.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.') + '$';
      if (new RegExp(pattern, 'i').test(host)) return 'DIRECT';
    }
  }
  if (scheme === 'ws') scheme = 'http';
  if (scheme === 'wss') scheme = 'https';
  return config.routes.all || config.routes[scheme] || config.routes.socks || 'DIRECT';
}

export function buildPac(system, port = 17890) {
  if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('守卫代理端口无效');
  const config = parseSystemProxy(system);
  return '// ClaudeGuard persistent fail-closed route v1\nfunction FindProxyForURL(url, host) { return (' + route.toString() + ')(url, host, ' + JSON.stringify(config) + ', ' + port + ', ' + JSON.stringify(protectedDomains) + '); }';
}
