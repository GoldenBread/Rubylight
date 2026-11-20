import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer({ configFile: false });
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { patchValue, normalise, describeDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { toDraft, changes } = await server.ssrLoadModule('/src/components/devices/device.ts');
const shipped = new Map(schema.settings.map(setting => [setting.key, setting]));
const client = extra => ({ uuid: 'u', name: 'TV', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('the couch menu has its own group in Sound & controls, with the host defaults', () => {
  const keys = ['couch_menu', 'couch_menu_open_with', 'couch_menu_controller_chord', 'couch_menu_hold_ms',
    'couch_menu_corner_hold_ms', 'couch_menu_keyboard_chord'];
  assert.deepEqual(keys.map(key => shipped.get(key)?.group), keys.map(() => 'Couch menu'));
  assert.deepEqual(keys.map(key => shipped.get(key).default),
    [true, 'controller,keyboard,touch', 'l3+r3', 1000, 1500, 'ctrl+alt+backspace']);
  assert.equal(shipped.get('couch_menu').control.kind, 'toggle');
  assert.equal(shipped.get('couch_menu').advanced, undefined);
});

test('the ways to open the menu are one switch each, and none stays an explicit empty list', () => {
  const setting = shipped.get('couch_menu_open_with');
  assert.deepEqual(setting.control.options.map(option => option.value), ['controller', 'keyboard', 'touch']);
  assert.deepEqual(normalise(setting, 'controller,touch'), ['controller', 'touch']);
  assert.equal(patchValue(setting, ['controller', 'touch']), 'controller,touch');
  // An empty value is dropped by the configuration file and means the default there.
  assert.equal(patchValue(setting, []), ',');
  assert.deepEqual(normalise(setting, ','), []);
  assert.match(describeDefault(setting).text, /^Controller chord, Keyboard chord, Top-left/);
});

test('night mode and headphone surround are host-wide defaults under Sound', () => {
  for (const key of ['audio_night_mode', 'audio_headphone_surround']) {
    const setting = shipped.get(key);
    assert.deepEqual([setting.category, setting.group, setting.default, setting.control.kind],
      ['audio', 'Sound', false, 'toggle'], key);
  }
});

test('a device sets night mode and headphone surround beside its other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000 } }));
  assert.deepEqual(before.audio, { audio_night_mode: '', audio_headphone_surround: '' });
  assert.deepEqual(changes(before, { ...before, audio: { ...before.audio, audio_night_mode: 'on' } }), {
    config_overrides: { max_bitrate: 20000, audio_night_mode: true },
  });
  const stored = toDraft(client({ config_overrides: { audio_headphone_surround: false } }));
  assert.equal(stored.audio.audio_headphone_surround, 'off');
  assert.equal('audio_headphone_surround' in stored.other_overrides, false);
  assert.deepEqual(changes(stored, { ...stored, name: 'Cube' }), { name: 'Cube' });
  assert.deepEqual(changes(stored, { ...stored, audio: { ...stored.audio, audio_headphone_surround: '' } }),
    { config_overrides: null });
});
