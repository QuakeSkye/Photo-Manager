import { readFile } from 'node:fs/promises';

import { expect, test } from 'vitest';

test('the default Tauri capability grants only core UI and native configuration pickers', async () => {
  const text = await readFile(
    new URL('../../src-tauri/capabilities/default.json', import.meta.url),
    'utf8',
  );
  const capability = JSON.parse(text) as { permissions: string[] };

  expect(capability.permissions).toEqual(['core:default', 'dialog:allow-open']);
  expect(capability.permissions.join(' ')).not.toMatch(/(?:fs|shell|upload)/i);
});

test('the frontend exposes narrow id-based photo and external-tool commands', async () => {
  const text = await readFile(
    new URL('../../src/shared/commands/safety.ts', import.meta.url),
    'utf8',
  );

  expect(text).not.toMatch(/\bpath\b/i);
  expect(text).not.toMatch(/plugin-(fs|shell)/i);

  const libraryClient = await readFile(
    new URL('../../src/shared/commands/library.ts', import.meta.url),
    'utf8',
  );
  expect(libraryClient).not.toMatch(
    /invoke[^\n]+(?:execute_delete|recycle|create_deletion_batch|import|photoshop|file_location)/i,
  );
  expect(libraryClient).toMatch(
    /invoke<DeletionMutationResult>\('toggle_deletion_mark', \{\s*photoId,\s*photoVersion,\s*operationId,/,
  );
  expect(libraryClient).toMatch(
    /invoke<DeletionMutationResult>\('undo_deletion_mark', \{ operationId \}\)/,
  );
  expect(libraryClient).toMatch(
    /invoke<\{ dataUrl: string \}>\('get_photo_preview', \{[\s\S]*?assetId: reference\.assetId,[\s\S]*?assetVersion: reference\.assetVersion/,
  );

  const settingsClient = await readFile(
    new URL('../../src/shared/commands/settings.ts', import.meta.url),
    'utf8',
  );
  expect(settingsClient).not.toMatch(/plugin-(fs|shell)/i);
  expect(settingsClient).toMatch(
    /invoke\('open_raf_in_photoshop', \{ toolId, photoId, photoVersion \}\)/,
  );
  expect(settingsClient).toMatch(
    /invoke\('open_photo_file_location', \{ photoId, photoVersion \}\)/,
  );
  expect(settingsClient).toMatch(
    /invoke<PhotoshopStatus \| null>\('choose_photoshop'\)/,
  );
  expect(settingsClient).not.toMatch(
    /open_raf_in_photoshop[^\n]+(?:path|executable)/i,
  );
});
