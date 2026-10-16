// Regenerate the Windows ICO and PNG from brand-mark.svg using an isolated Edge renderer.
// Run from the repository: node docs/generate-icons.mjs (Node.js 22+ and Edge required).
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const docs = path.dirname(fileURLToPath(import.meta.url));
const root = path.dirname(docs);
const assets = path.join(root, 'rust/crates/desktop/assets');
const qa = path.join(root, 'rust/dist/qa/icons');
await mkdir(assets, { recursive: true });
await mkdir(qa, { recursive: true });
const source = await readFile(path.join(docs, 'brand-mark.svg'), 'utf8');
const profile = await mkdtemp(path.join(qa, 'renderer-'));
const executable = process.env.MELIAN_BROWSER || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe';
const child = spawn(executable, ['--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--disable-sync', '--disable-background-networking', '--remote-debugging-port=0', '--user-data-dir=' + profile, 'about:blank'], { windowsHide: true, stdio: 'ignore' });
let launchError;
child.on('error', error => { launchError = error; });
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const sockets = [];
async function connect(url) {
  const socket = new WebSocket(url), pending = new Map(); let sequence = 0;
  sockets.push(socket);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  socket.onmessage = event => {
    const response = JSON.parse(event.data), task = pending.get(response.id);
    if (!task) return;
    pending.delete(response.id); clearTimeout(task.timer);
    response.error ? task.reject(new Error(JSON.stringify(response.error))) : task.resolve(response.result);
  };
  return { send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => { pending.delete(id); reject(new Error('Renderer timeout: ' + method)); }, 10000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  } };
}
let browser;
try {
  let port;
  for (let attempt = 0; attempt < 60; attempt++) {
    if (launchError) throw launchError;
    try { port = Number((await readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); break; }
    catch { await sleep(250); }
  }
  if (!port) throw new Error('Could not start the isolated icon renderer');
  const version = await (await fetch('http://127.0.0.1:' + port + '/json/version')).json();
  browser = await connect(version.webSocketDebuggerUrl);
  const pages = await (await fetch('http://127.0.0.1:' + port + '/json/list')).json();
  const page = await connect(pages.find(item => item.type === 'page').webSocketDebuggerUrl);
  const sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512];
  const frames = [];
  for (const size of sizes) {
    let svg = source;
    // Optical correction: stronger strokes, no one-pixel star at the smallest sizes.
    if (size <= 24) svg = svg.replace('stroke-width="19"', 'stroke-width="23"');
    if (size <= 20) svg = svg.replace(/<path id="star"[^>]*\/>/, '');
    const sourceUrl = 'data:image/svg+xml;base64,' + Buffer.from(svg).toString('base64');
    const result = await page.send('Runtime.evaluate', {
      expression: `(async()=>{const img=new Image();img.src=${JSON.stringify(sourceUrl)};await img.decode();const canvas=document.createElement('canvas');canvas.width=canvas.height=${size};canvas.getContext('2d').drawImage(img,0,0,${size},${size});return canvas.toDataURL('image/png')})()`,
      awaitPromise: true, returnByValue: true,
    });
    if (result.exceptionDetails || !result.result.value?.startsWith('data:image/png;base64,')) throw new Error('SVG rendering failed');
    const dataUrl = result.result.value;
    const png = Buffer.from(dataUrl.split(',')[1], 'base64');
    if (png.readUInt32BE(16) !== size || png.readUInt32BE(20) !== size) throw new Error('Unexpected raster dimensions');
    frames.push({ size, png, dataUrl });
    await writeFile(path.join(qa, `melian-${size}.png`), png);
  }
  const iconFrames = frames.filter(frame => frame.size <= 256);
  const header = Buffer.alloc(6 + iconFrames.length * 16);
  header.writeUInt16LE(1, 2); header.writeUInt16LE(iconFrames.length, 4);
  let offset = header.length;
  iconFrames.forEach(({ size, png }, index) => {
    const entry = 6 + index * 16;
    header[entry] = header[entry + 1] = size === 256 ? 0 : size;
    header.writeUInt16LE(1, entry + 4); header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(png.length, entry + 8); header.writeUInt32LE(offset, entry + 12);
    offset += png.length;
  });
  await writeFile(path.join(assets, 'melian.ico'), Buffer.concat([header, ...iconFrames.map(frame => frame.png)]));
  await writeFile(path.join(docs, 'brand-mark.png'), frames.at(-1).png);

  const row = frames.filter(frame => [16, 24, 32, 48, 64].includes(frame.size)).map(frame => `<div><img src="${frame.dataUrl}" width="${frame.size}" height="${frame.size}"><small>${frame.size} px</small></div>`).join('');
  const html = `<html><meta charset="utf-8"><style>*{box-sizing:border-box}body{margin:0;background:#f3f5f3;color:#182623;font-family:Segoe UI,Microsoft YaHei,sans-serif}main{padding:48px}header{display:flex;align-items:center;gap:48px;margin-bottom:36px}h1{font-size:52px;letter-spacing:-2px;margin:0 0 12px}p{color:#5b6c63;margin:0;font-size:17px}.grid{display:grid;grid-template-columns:1fr 1fr;gap:20px}.card{height:170px;border-radius:20px;padding:26px;background:#fff}.dark{background:#151c1d;color:#dfeae3}.row{display:flex;gap:26px;align-items:end;height:110px}.row div{display:flex;align-items:center;flex-direction:column;gap:14px}small{font-size:12px;opacity:.6}.label{font-size:12px;letter-spacing:2px;opacity:.65}</style><main><header><img src="${frames.at(-1).dataUrl}" width="232" height="232"><div><h1>Melian</h1><p>双拱为门，星光在内。</p><p style="margin-top:12px;font-size:14px">应用图标 · 桌面 · 开始菜单 · 托盘</p></div></header><div class="grid"><section class="card"><div class="label">LIGHT</div><div class="row">${row}</div></section><section class="card dark"><div class="label">DARK</div><div class="row">${row}</div></section></div></main></html>`;
  await writeFile(path.join(qa, 'preview.html'), html);
  await page.send('Emulation.setDeviceMetricsOverride', { width: 1000, height: 550, deviceScaleFactor: 1, mobile: false });
  await page.send('Page.enable');
  const frameId = (await page.send('Page.getFrameTree')).frameTree.frame.id;
  await page.send('Page.setDocumentContent', { frameId, html });
  await page.send('Runtime.evaluate', { expression: 'Promise.all([...document.images].map(img=>img.decode()))', awaitPromise: true });
  const shot = await page.send('Page.captureScreenshot', { format: 'png' });
  await writeFile(path.join(qa, 'preview.png'), Buffer.from(shot.data, 'base64'));
  console.log('Generated melian.ico: ' + iconFrames.map(frame => frame.size).join(', ') + ' px');
  console.log('Preview: ' + path.join(qa, 'preview.png'));
} finally {
  if (browser) { try { await browser.send('Browser.close'); } catch {} }
  for (const socket of sockets) socket.close();
  child.kill();
}
