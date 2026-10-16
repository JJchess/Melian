// Keep this function standalone and ES5-compatible for Chromium's PAC runtime.
function claudeGuardRoute(url, host, config, guardPort) {
    host = host.toLowerCase().replace(/^\[|\]$/g, '');
    var protectedHost = host.replace(/\.$/, '');
    var domains = ['claude.ai', 'claude.com', 'anthropic.com'];
    if (protectedHost === 'connect.claudeguard.invalid') return 'PROXY 127.0.0.1:' + guardPort;
    for (var i = 0; i < domains.length; i++) {
        if (protectedHost === domains[i] || protectedHost.slice(-(domains[i].length + 1)) === '.' + domains[i]) {
            return 'PROXY 127.0.0.1:' + guardPort;
        }
    }
    var match = /^([a-z]+):\/\/([^/]+)/i.exec(url);
    var scheme = match ? match[1].toLowerCase() : 'https';
    var authority = match ? match[2].replace(/^.*@/, '') : host;
    var portMatch = /:([0-9]+)$/.exec(authority);
    var port = portMatch ? Number(portMatch[1]) : scheme === 'https' || scheme === 'wss' ? 443 : scheme === 'ftp' ? 21 : 80;
    // Chromium also performs these implicit bypasses before evaluating a PAC.
    if (protectedHost === '::1' || /^127\./.test(host) || /^169\.254\./.test(host) ||
        /^fe[89ab][0-9a-f]:/i.test(host) || protectedHost === 'localhost' ||
        /\.localhost$/.test(protectedHost) || protectedHost === 'loopback') return 'DIRECT';
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
            var address = ipNumber(host), network = ipNumber(rule.network);
            var mask = rule.prefix ? (0xffffffff << (32 - rule.prefix)) >>> 0 : 0;
            if (address !== null && (address & mask) === (network & mask)) return 'DIRECT';
        }
        if (rule.host && (!rule.scheme || scheme === rule.scheme) && (!rule.port || port === rule.port)) {
            var pattern = '^' + rule.host.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.') + '$';
            if (new RegExp(pattern, 'i').test(host)) return 'DIRECT';
        }
    }
    if (scheme === 'ws' || scheme === 'wss') {
        return config.routes.all || config.routes.socks || config.routes.https || config.routes.http || 'DIRECT';
    }
    return config.routes.all || config.routes[scheme] || config.routes.socks || 'DIRECT';
}
