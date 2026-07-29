import { readFile } from 'node:fs/promises';

import { expect, test } from 'vitest';

test('the default Tauri capability contains no filesystem or shell grants', async () => {
  const text = await readFile(
    new URL('../../src-tauri/capabilities/default.json', import.meta.url),
    'utf8',
  );
  const capability = JSON.parse(text) as { permissions: string[] };

  expect(capability.permissions).toEqual(['core:default']);
});

test('the frontend command client exposes no path-based write command', async () => {
  const text = await readFile(
    new URL('../../src/shared/commands/safety.ts', import.meta.url),
    'utf8',
  );

  expect(text).not.toMatch(/\bpath\b/i);
  expect(text).not.toMatch(/plugin-(fs|shell)/i);
});
