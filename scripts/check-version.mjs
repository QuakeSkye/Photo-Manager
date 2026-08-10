import { readFileSync } from 'node:fs';
import { URL } from 'node:url';

const read = (path) =>
  readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const packageJson = JSON.parse(read('package.json'));
const packageLock = JSON.parse(read('package-lock.json'));
const tauriConfig = JSON.parse(read('src-tauri/tauri.conf.json'));
const cargoToml = read('src-tauri/Cargo.toml');
const cargoLock = read('src-tauri/Cargo.lock');

const expected = packageJson.version;
const versions = new Map([
  ['package-lock root', packageLock.version],
  ['package-lock workspace', packageLock.packages?.['']?.version],
  ['Tauri config', tauriConfig.version],
  ['Cargo package', packageVersion(cargoToml, 'Cargo.toml')],
  ['Cargo lock package', packageVersion(cargoLock, 'Cargo.lock')],
]);

const mismatches = [...versions].filter(([, version]) => version !== expected);
if (mismatches.length > 0) {
  const detail = mismatches
    .map(([source, version]) => `${source}=${String(version)}`)
    .join(', ');
  throw new Error(`版本号不一致：package.json=${expected}; ${detail}`);
}

console.log(`版本号一致：${expected}`);

function packageVersion(content, label) {
  const pattern =
    label === 'Cargo.toml'
      ? /^\[package\]\r?\n(?:[^\r\n]*\r?\n)*?version = "([^"]+)"/m
      : /^\[\[package\]\]\r?\nname = "local-photo-manager"\r?\nversion = "([^"]+)"/m;
  const version = content.match(pattern)?.[1];
  if (version === undefined) throw new Error(`${label} 缺少应用包版本。`);
  return version;
}
