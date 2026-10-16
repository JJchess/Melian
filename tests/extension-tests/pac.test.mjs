import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { buildPac, isProtectedUrl, parseSystemProxy } from '../../extension/pac.mjs';

const system = { supported: true, enabled: true, server: '127.0.0.1:7897', bypass: '<local>;*.internal;127.*;claude.ai;*.anthropic.com' };
function compile(config = system, port = 17890) {
  const context = vm.createContext({});
  vm.runInContext(buildPac(config, port), context);
  return (url, host = new URL(url).hostname) => context.FindProxyForURL(url, host);
}

test('protected domains always use guard, including bypass matches, trailing dots and websockets', () => {
  const route = compile();
  for (const url of ['https://claude.ai/', 'http://claude.com/', 'https://api.anthropic.com/v1/messages', 'wss://a.b.claude.ai/chat', 'https://CLAUDE.AI./']) {
    assert.equal(route(url), 'PROXY 127.0.0.1:17890', url);
    assert.ok(isProtectedUrl(url));
  }
  assert.equal(compile(system, 23456)('https://claude.ai/'), 'PROXY 127.0.0.1:23456');
});

test('suffix attacks and unrelated domains follow actual static system proxy', () => {
  const route = compile();
  for (const host of ['notclaude.ai', 'claude.ai.attacker.test', 'claude-ai.test', 'anthropic.com.evil', 'example.com']) {
    assert.equal(route('https://' + host), 'PROXY 127.0.0.1:7897');
    assert.equal(isProtectedUrl('https://' + host), false);
  }
  assert.equal(isProtectedUrl('javascript:alert(1)'), false);
  assert.equal(isProtectedUrl('file://claude.ai/a'), false);
});

test('disabled system proxy directs only unrelated websites directly', () => {
  const route = compile({ supported: true, enabled: false });
  assert.equal(route('https://example.com'), 'DIRECT');
  assert.equal(route('https://claude.ai'), 'PROXY 127.0.0.1:17890');
});

test('protocol mappings, default ports and SOCKS fallback preserve Windows static settings', () => {
  const route = compile({ supported: true, enabled: true, server: 'http=proxy.test:80;https=https://secure.test;socks=socks5://socks.test' });
  assert.equal(route('http://example.com'), 'PROXY proxy.test:80');
  assert.equal(route('https://example.com'), 'HTTPS secure.test:443');
  assert.equal(route('ws://example.com'), 'PROXY proxy.test:80');
  assert.equal(route('wss://example.com'), 'HTTPS secure.test:443');
  assert.equal(route('ftp://example.com'), 'SOCKS5 socks.test:1080');
  assert.equal(compile({ supported: true, enabled: true, server: 'http=proxy.test:81' })('https://example.com'), 'DIRECT');
});

test('local, wildcards, literal punctuation, CIDR and scheme/port bypasses are accurate', () => {
  const route = compile({ ...system, bypass: '<local>;*.internal;https://x.example:8443;10.2.0.0/16;[::1];literal.example.test' });
  for (const url of ['http://printer', 'https://a.internal', 'https://x.example:8443', 'http://10.2.99.1', 'http://[::1]', 'http://localhost', 'http://127.2.3.4', 'http://literal.example.test']) assert.equal(route(url), 'DIRECT', url);
  for (const url of ['https://x.example', 'http://x.example:8443', 'http://10.3.1.1', 'http://literalXexample.test']) assert.equal(route(url), 'PROXY 127.0.0.1:7897', url);
  assert.equal(compile({ ...system, bypass: '<-loopback>' })('http://127.0.0.1'), 'PROXY 127.0.0.1:7897');
});

test('invalid and unsupported configurations fail closed instead of silently assuming direct', () => {
  for (const config of [null, { supported: false, reason: 'PAC unsupported' }, { supported: true, enabled: true },
    { ...system, server: 'proxy:99999' }, { ...system, server: 'http=user:password@proxy:80' },
    { ...system, server: 'http=a:80;http=b:80' }, { ...system, bypass: '10.1.1.1/99' },
    { ...system, bypass: 'arbitrary<script>' }, { ...system, server: 'proxy:80; DIRECT' }]) assert.throws(() => buildPac(config));
  for (const port of [0, -1, 80, 65536, '17890', 17890.5]) assert.throws(() => buildPac(system, port));
  assert.doesNotThrow(() => parseSystemProxy({ supported: true, enabled: false, server: 'ignored' }));
});

test('manifest key resolves to the registered native-messaging extension ID', async () => {
  const manifest = JSON.parse(await readFile(new URL('../../extension/manifest.json', import.meta.url), 'utf8'));
  const hash = createHash('sha256').update(Buffer.from(manifest.key, 'base64')).digest().subarray(0, 16);
  const id = [...hash].map(b => String.fromCharCode(97 + (b >> 4), 97 + (b & 15))).join('');
  assert.equal(id, (await readFile(new URL('../../extension/extension-id.txt', import.meta.url), 'utf8')).trim());
  assert.equal(manifest.incognito, 'not_allowed');
  assert.equal(manifest.declarative_net_request.rule_resources[0].enabled, true);
});
