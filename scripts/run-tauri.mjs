import { spawnSync } from 'node:child_process';
import { delimiter, join } from 'node:path';

const cargoBin = join(process.env.USERPROFILE ?? '', '.cargo', 'bin');
const tauriScript = join(
  process.cwd(),
  'node_modules',
  '@tauri-apps',
  'cli',
  'tauri.js',
);
const result = spawnSync(
  process.execPath,
  [tauriScript, ...process.argv.slice(2)],
  {
    cwd: process.cwd(),
    stdio: 'inherit',
    env: {
      ...process.env,
      PATH: `${cargoBin}${delimiter}${process.env.PATH ?? ''}`,
    },
  },
);

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
