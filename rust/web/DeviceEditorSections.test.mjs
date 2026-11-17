import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const DeviceEditor = (await server.ssrLoadModule('/src/components/devices/DeviceEditor.svelte')).default;
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });
const section = (body, name) => new RegExp(`<details class="override[^>]*><summary[^>]*>${name}`).exec(body)[0];
const editor = extra => render(DeviceEditor, { props: { client: client(extra), sleepUnavailable: null } }).body;

test('no section of the device editor closes under the cursor', async () => {
  // Display overrides followed `open={modeInvalid}`: typing a mode that turned valid shut it.
  const source = await readFile(new URL('./src/components/devices/DeviceEditor.svelte', import.meta.url), 'utf8');
  const tags = source.match(/<details\b[^>]*>/g);
  assert.ok(tags.length >= 5, String(tags.length));
  for (const tag of tags) assert.doesNotMatch(tag, /(?<!bind:)\bopen=\{/, tag);
});

test('an invalid display mode or an unknown preset still opens its section', () => {
  assert.match(section(editor({ display_mode: '1920x' }), 'Display overrides'), /\sopen/);
  assert.doesNotMatch(section(editor({ display_mode: '' }), 'Display overrides'), /\sopen/);
  assert.match(section(editor({ config_overrides: { stream_preset: 'gone' } }), 'Stream preset'), /\sopen/);
  assert.doesNotMatch(section(editor({}), 'Stream preset'), /\sopen/);
});

test('the display scale says a scale the device asks for comes first', () => {
  // Nightjar's PC display scaling (rlDisplayScale) wins over this override on the virtual display.
  assert.match(editor({}), /A scale the device asks for itself \(Nightjar(&#39;|')s PC display scaling\) comes first on its virtual display\./);
});

test('a layout a stream learned is named on the device page, with a way to clear it', () => {
  // Issue #40: the host saved a layout a stream ended in; the page says so instead of showing it as the user's.
  const learned = editor({
    virtual_display_layout: 'extended_primary',
    virtual_display_layout_learned_over: 'extended',
  });
  assert.match(learned, /Learned from your last stream: Extend — make virtual display primary\./);
  assert.match(learned, /<button[^>]*>Clear<\/button>/);
  // A layout the user picked, or none at all, has nothing learned to say.
  assert.doesNotMatch(
    editor({
      virtual_display_layout: 'extended_primary',
      virtual_display_layout_learned_over: 'extended',
      virtual_display_layout_set_by_user: true,
    }),
    /Learned from your last stream/,
  );
  assert.doesNotMatch(editor({}), /Learned from your last stream/);
});
