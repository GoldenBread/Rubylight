import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { toDraft, changes, validReleaseDelay } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const DeviceEditor = (await server.ssrLoadModule('/src/components/devices/DeviceEditor.svelte')).default;
const client = extra => ({ uuid: 'u', name: 'Tablet', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

// A touch or pen problem is fixed per device, so the delay is a device override of its own
// (absolute_left_release_delay), and 0 is a value rather than "use the host setting".
test('a device sets its own touch and pen click delay, beside its other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000 } }));
  assert.equal(before.release_delay, '');
  assert.deepEqual(changes(before, { ...before, release_delay: '0' }), {
    config_overrides: { max_bitrate: 20000, absolute_left_release_delay: 0 },
  });
  assert.deepEqual(changes(before, { ...before, release_delay: '40' }), {
    config_overrides: { max_bitrate: 20000, absolute_left_release_delay: 40 },
  });
});

test('a stored delay is read back, and the host setting removes it', () => {
  const before = toDraft(client({ config_overrides: { absolute_left_release_delay: 25 } }));
  assert.equal(before.release_delay, '25');
  assert.equal('absolute_left_release_delay' in before.other_overrides, false);
  assert.deepEqual(changes(before, { ...before, release_delay: '' }), { config_overrides: null });
  // Untouched, it goes back as stored.
  assert.deepEqual(changes(before, { ...before, name: 'Tablet 2' }), { name: 'Tablet 2' });
});

test('only a whole number of milliseconds up to 1000 is a delay', () => {
  for (const value of ['', ' ', '0', '10', '1000']) assert.equal(validReleaseDelay(value), true, value);
  for (const value of ['1001', '-1', '1.5', 'soon', '10000']) assert.equal(validReleaseDelay(value), false, value);
});

test('the editor shows the delay and opens on a value it cannot use', () => {
  const body = props => render(DeviceEditor, { props: { client: client(props), sleepUnavailable: null } }).body;
  const section = html => /<details class="override[^>]*><summary[^>]*>Touch and pen</.exec(html)[0];
  assert.match(body({}), /Touch and pen click delay/);
  assert.doesNotMatch(section(body({})), /\sopen/);
  assert.doesNotMatch(section(body({ config_overrides: { absolute_left_release_delay: 20 } })), /\sopen/);
  assert.match(section(body({ config_overrides: { absolute_left_release_delay: 'soon' } })), /\sopen/);
});
