import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const files = ['pac.test.mjs', 'worker.test.mjs'].map(name => fileURLToPath(new URL(name, import.meta.url)));
const result = spawnSync(process.execPath, ['--experimental-vm-modules', '--test', ...files], { stdio: 'inherit', windowsHide: true });
process.exitCode = result.status ?? 1;
