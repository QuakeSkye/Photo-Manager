import { spawnSync } from 'node:child_process';
import { join } from 'node:path';

const cargoExecutable = join(
  process.env.USERPROFILE ?? '',
  '.cargo',
  'bin',
  'cargo.exe',
);
const result = spawnSync(cargoExecutable, process.argv.slice(2), {
  cwd: process.cwd(),
  stdio: 'inherit',
  env: process.env,
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
