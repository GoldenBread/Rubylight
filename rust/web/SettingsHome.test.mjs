import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { schema, categoriesOf, settingsGroup, groupAnchor } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const main = await server.ssrLoadModule('/src/lib/schema/main.ts');
const { searchSettings } = await server.ssrLoadModule('/src/components/settings/search.ts');
const { settings: extensions } = await server.ssrLoadModule('/src/lib/schema/extensions.ts');
const { describeDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { presets } = await server.ssrLoadModule('/src/lib/stream-presets.ts');
const { MAIN_SECTIONS, MAIN_KEYS, MAIN_BUDGET, ALWAYS_ON, DIAGNOSTICS, TRAILING_GROUPS, DEBUG_GROUP, onHome, presetOutcome } = main;
const { VIRTUAL_DISPLAY_OS } = await server.ssrLoadModule('/src/lib/settings-types.ts');
const settings = new Map(schema.settings.map(setting => [setting.key, setting]));
const source = name => readFile(new URL(`./src/${name}`, import.meta.url), 'utf8');

test('every main key is a live schema setting, listed once', () => {
  assert.equal(new Set(MAIN_KEYS).size, MAIN_KEYS.length);
  for (const key of MAIN_KEYS) {
    const setting = settings.get(key);
    assert.ok(setting, `${key} is in the schema`);
    assert.notEqual(setting.pending, true, key);
    // The home's top level shows it without a parent choice turning it on first.
    assert.equal(setting.visibleWhen?.({}) ?? true, true, key);
    assert.equal(ALWAYS_ON.includes(key), false, key);
    assert.equal(TRAILING_GROUPS.has(setting.group), false, key);
  }
});

test('the Settings home stays within its budget of 12 settings', () => {
  assert.equal(MAIN_BUDGET, 12);
  assert.ok(MAIN_KEYS.length <= MAIN_BUDGET, `${MAIN_KEYS.length} main settings`);
  assert.deepEqual(MAIN_SECTIONS.map(section => section.title), ['This PC', 'Picture', 'Sound & controls', 'Games']);
  // Each home section is an All settings tab of the same name, which its "More settings" link opens.
  for (const section of MAIN_SECTIONS) assert.equal(settingsGroup(section.id), section.id, section.id);
});

test('main settings are safe in every option: nothing Experimental on the home', () => {
  for (const key of MAIN_KEYS) {
    const setting = onHome(settings.get(key), settings.get(key).default);
    const options = setting.control.kind === 'select' ? setting.control.options.map(option => option.label) : [];
    assert.doesNotMatch([setting.label, setting.description, ...options].join(' '), /Experimental/, key);
  }
});

test('the monitor layout reads as an outcome and never rewrites another layout', () => {
  const layout = settings.get('virtual_display_layout');
  const plain = onHome(layout, 'exclusive');
  assert.equal(plain.key, 'virtual_display_layout');
  assert.equal(plain.label, 'While streaming, your monitors');
  assert.deepEqual(plain.control.options.map(option => [option.value, option.label]), [['exclusive', 'Turn off'], ['extended', 'Stay on']]);
  // A layout chosen in All settings stays a choice under its own name.
  const isolated = onHome(layout, 'extended_isolated');
  assert.deepEqual(isolated.control.options.map(option => option.value), ['exclusive', 'extended', 'extended_isolated']);
  assert.match(isolated.control.options[2].label, /^Stay on: Isolate/);
  // Every value the home offers is one the host reads.
  for (const option of isolated.control.options) assert.ok(layout.control.options.some(item => item.value === option.value));
  assert.equal(layout.default, 'exclusive');
});

test('the home offers the resolution choices that need no other field, and names a stored fixed one', async () => {
  const resolution = settings.get('dd_resolution_option');
  const plain = onHome(resolution, 'auto', {});
  assert.equal(plain.label, 'Stream resolution');
  assert.deepEqual(plain.control.options.map(option => [option.value, option.label]), [
    ['auto', 'Match the device'],
    ['disabled', 'Keep the current resolution'],
  ]);
  // A stored fixed resolution shows what it is; the home cannot create an empty one.
  const manual = onHome(resolution, 'manual', { dd_manual_resolution: '2560x1440' });
  assert.deepEqual(manual.control.options.at(-1), { value: 'manual', label: 'Fixed: 2560 × 1440' });
  const unnamed = onHome(resolution, 'manual', {});
  assert.deepEqual(unnamed.control.options.at(-1), { value: 'manual', label: 'Fixed resolution: set it in All settings' });
  // Every value the home offers is one the host reads.
  for (const option of manual.control.options) assert.ok(resolution.control.options.some(item => item.value === option.value));
  assert.equal(onHome(resolution, 'something-else', {}).control.options.length, 2);
  // The note's target is a real Picture group, so the link opens the field it names.
  assert.equal(groupAnchor('Resolution and refresh rate'), 'resolution-and-refresh-rate');
  const group = schema.settings.find(setting => setting.group === 'Resolution and refresh rate');
  assert.equal(settingsGroup(group.category), 'picture');
  const home = await source('pages/SettingsHome.svelte');
  assert.match(home, /inAllSettings\('picture', 'Resolution and refresh rate'\)/);
});

test('the stream preset reads as a trade-off on the home, with plain outcomes', async () => {
  const preset = onHome(settings.get('stream_preset'), 'balanced');
  assert.match(preset.description, /delay and picture quality/);
  assert.doesNotMatch(preset.description, /encoder settings|\bbelow\b/);
  assert.deepEqual(preset.control.options.map(option => option.label), ['Lowest latency', 'Balanced (recommended)', 'Reference quality']);
  assert.equal(presetOutcome('balanced'), 'Rubylight’s defaults: the safest choice for almost every game.');
  assert.equal(presetOutcome('lowest_latency'), 'Handles each frame as fast as it can. Some picture quality is lost.');
  assert.equal(presetOutcome('reference'),
    'Removes the size limit on recovery frames, so a loss costs more bandwidth; no picture-quality gain from it has been measured. Use it only on a fast, steady link.');
  // A preset added to the shared table without its own sentence fails here
  // instead of being labelled as Balanced on the home.
  for (const entry of presets) assert.ok(presetOutcome(entry.id), entry.id);
  const home = await source('pages/SettingsHome.svelte');
  assert.match(home, /\{@const outcome = presetOutcome\(preset\.id\)\}/);
  assert.match(home, /\{#if outcome\}<p class="note">\{outcome\}<\/p>\{\/if\}/);
  assert.doesNotMatch(home, /\{preset\.description\}/);
});

test('the home says what Default virtual display does on this PC', async () => {
  const mode = settings.get('virtual_display_mode');
  const able = onHome(mode, '', { [VIRTUAL_DISPLAY_OS]: true }, { capable: true, ready: true });
  assert.match(able.description, /makes one for each device/);
  // A device or app can still choose its own mode.
  assert.match(able.description, /a device’s or app’s choice wins/);
  // A host that does not say counts as able, as its own virtual display gate does.
  assert.match(onHome(mode, '', {}).description, /makes one for each device/);
  assert.match(onHome(mode, '', { [VIRTUAL_DISPLAY_OS]: false }).description, /cannot run Rubylight’s virtual display driver/);
  // The driver decides too: installed but not ready, and missing, promise no screen.
  assert.match(onHome(mode, '', {}, { capable: true, ready: false }).description, /once the driver is ready/);
  assert.match(onHome(mode, '', {}, { capable: false, ready: false }).description, /is not available on this PC/);
  // A chosen mode keeps the setting's own words.
  assert.equal(onHome(mode, 'shared', { [VIRTUAL_DISPLAY_OS]: true }).description, mode.description);
  // The home gives the transform the driver the host reported, not only the OS gate.
  const home = await source('pages/SettingsHome.svelte');
  assert.match(home, /host\.metadata\?\.virtual_display/);
});

test('the Settings home search says when it lists only the first matches', async () => {
  const home = await source('pages/SettingsHome.svelte');
  assert.match(home, /Showing \{RESULTS\} of \{results\.length\} settings/);
  assert.match(home, /const RESULTS = 8;/);
});

test('every schema key is reachable from search, and opens its own category', () => {
  for (const setting of schema.settings) {
    const found = searchSettings(schema.settings, categoriesOf, setting.key);
    assert.ok(found.includes(setting), `${setting.key} is found by its key`);
    const byLabel = searchSettings(schema.settings, categoriesOf, setting.label);
    assert.ok(byLabel.includes(setting), `${setting.key} is found by its label`);
    assert.ok(categoriesOf(setting).some(category => category.id === settingsGroup(setting.category)), setting.key);
  }
  assert.deepEqual(searchSettings(schema.settings, categoriesOf, '   '), []);
});

test('always-on keys appear only in Debug switches, described as for troubleshooting', () => {
  assert.deepEqual([...ALWAYS_ON].sort(), [
    'audio_passthrough', 'audio_surround_714', 'bandwidth_probe', 'client_roaming', 'frame_warp', 'gpu_compute_conversion',
    'hdr_dolby_vision', 'hdr_hlg', 'idd_push_fence_ring', 'keyframe_loss_pacing',
    'nvenc_frame_budget_guard', 'nvenc_realtime_hags', 'packed_444', 'pyrowave', 'qr_pairing',
    'stream_reconfigure', 'wgc_direct_encoder_input', 'wgc_pacing_smoothing',
  ]);
  for (const key of ALWAYS_ON) {
    const setting = settings.get(key);
    assert.ok(setting, `${key} stays readable in All settings`);
    assert.equal(setting.group, DEBUG_GROUP, key);
    assert.equal(settingsGroup(setting.category), 'advanced', key);
    assert.match(setting.description, /only for troubleshooting/, key);
    assert.equal(setting.default, true, key);
    assert.equal(MAIN_KEYS.includes(key), false, key);
    // Where it came from is kept, so a turned-off one shows in its topic tab too.
    assert.ok(setting.origin?.category && setting.origin.category !== 'advanced', key);
  }
  const debug = schema.settings.filter(setting => setting.group === DEBUG_GROUP).map(setting => setting.key);
  assert.deepEqual(debug.sort(), [...ALWAYS_ON, ...DIAGNOSTICS].sort());
  // The key itself is untouched in the part file.
  assert.equal(extensions.find(setting => setting.key === 'qr_pairing').group, 'Pairing');
});

test('the trailing groups start collapsed, and each one exists', () => {
  assert.deepEqual([...TRAILING_GROUPS].sort(), ['AMD AMF', 'Capture troubleshooting', 'Console', DEBUG_GROUP, 'Key remapping', 'Mode substitutions', 'NVIDIA NVENC', 'Ports and addresses', 'RTSS'].sort());
  // A group shows in a topic tab under the name its settings came from, once one of them is changed.
  const groups = new Set(schema.settings.map(setting => (setting.origin ? setting.origin.group : setting.group)));
  for (const group of TRAILING_GROUPS) assert.ok(groups.has(group) || group === DEBUG_GROUP, group);
});

test('All settings has no second Advanced fold; the home owns updates and wake, All settings the detail and sign-in', async () => {
  const page = await source('pages/Settings.svelte');
  assert.doesNotMatch(page, /class="advanced"/);
  assert.doesNotMatch(page, /setting\.advanced/);
  assert.match(page, /TRAILING_GROUPS\.has\(item\.name\)/);
  // Wake detail, sign-in and the update schedule moved off the home into All settings.
  for (const panel of ['UpdatesPanel', 'PowerPanel', 'SessionsPanel', 'PasswordPanel']) {
    assert.match(page, new RegExp(`<${panel}`), panel);
  }
  const help = (await source('pages/Help.svelte')) + (await source('pages/Troubleshoot.svelte'));
  for (const panel of ['UpdatesPanel', 'PowerPanel', 'SessionsPanel', 'PasswordPanel']) {
    assert.doesNotMatch(help, new RegExp(`<${panel}`), panel);
  }
  const home = await source('pages/SettingsHome.svelte');
  for (const panel of ['UpdatesPanel', 'WakeRow', 'SaveBar']) {
    assert.match(home, new RegExp(`<${panel}`), panel);
  }
  for (const panel of ['PowerPanel', 'SessionsPanel', 'PasswordPanel']) {
    assert.doesNotMatch(home, new RegExp(`<${panel}`), panel);
  }
  // The home saves through the same PATCH of changed keys as All settings.
  assert.match(home, /computeChanges\(defaults, edits, stored, new Set\(covered\)\)/);
  assert.match(home, /api\.config\.patch\(body\)/);
  const app = await source('App.svelte');
  assert.match(app, /route\.path === '\/settings'\) return SettingsHome/);
  assert.match(app, /match\('\/settings\/:section'\)\) return Settings/);
  for (const name of ['pages/Settings.svelte', 'pages/Help.svelte', 'pages/Troubleshoot.svelte', 'components/devices/PairPanel.svelte']) {
    assert.doesNotMatch(await source(name), /maintenance#account/, name);
  }
});

test('a default says what it does, never just "Default"', () => {
  for (const setting of schema.settings) {
    assert.doesNotMatch(describeDefault(setting).text, /^Default/, setting.key);
  }
  assert.equal(describeDefault(settings.get('amd_vbaq')).text, 'Off for H.264, on for HEVC and AV1');
});

test('the wake status never spins silently: a slow first check says why, and the request has a deadline', async () => {
  const panel = await source('components/maintenance/PowerPanel.svelte');
  assert.match(panel, /<LoadState error=\{loadError\} what="the wake status" slow="[^"]+half a minute\."/);
  const state = await source('components/LoadState.svelte');
  assert.match(state, /setTimeout\(\(\) => \(lingering = true\), 3000\)/);
  // An explicit space: Svelte trims one at the start of an {#if} block.
  assert.match(state, /\{#if lingering\}\{' '\}\{slow\}\{\/if\}/);
  assert.match(await source('lib/api.ts'), /status: \(refresh = false\) => get<PowerStatus>\(`[^`]*`, ACTION_TIMEOUT_MS\)/);
});

test('full chroma applies when saved: no restart tag, text or notice', async () => {
  const packed = settings.get('packed_444');
  assert.notEqual(packed.restart, true);
  assert.doesNotMatch(`${packed.label} ${packed.description}`, /restart/i);
  // The restart notice follows the host's answer, which is false for a save of only live settings.
  for (const name of ['pages/Settings.svelte', 'pages/SettingsHome.svelte']) {
    assert.match(await source(name), /saved = result\.restart_required !== false;\r?\n\s*if \(!saved\) notify\('Settings saved\.', 'ok'\);/, name);
  }
});

test('the home offers the monitors choice only where streams can use a virtual display, gated as All settings is', async () => {
  const { withHostGates, virtualDisplayUsed } = await server.ssrLoadModule('/src/components/settings/values.ts');
  const { VIRTUAL_DISPLAY_OS, NATIVE_ENCODERS, NVIDIA_ENHANCEMENTS } = await server.ssrLoadModule('/src/lib/settings-types.ts');
  const metadata = (os) => ({
    gpus: [{ name: 'NVIDIA GeForce RTX 4070', vendor: 0x10de }],
    features: { pyrowave: false, virtual_display: os, virtual_display_os: os, rtx_hdr: true },
  });
  const old = withHostGates({ virtual_display_mode: '' }, metadata(false));
  assert.equal(old[VIRTUAL_DISPLAY_OS], false);
  assert.deepEqual(old[NATIVE_ENCODERS], ['nvenc']);
  assert.deepEqual(old[NVIDIA_ENHANCEMENTS], ['rtx_hdr']);
  // Windows 10 or 11 before 23H2: Default and Off stream a monitor.
  assert.equal(virtualDisplayUsed(old), false);
  assert.equal(virtualDisplayUsed({ ...old, virtual_display_mode: 'disabled' }), false);
  assert.equal(virtualDisplayUsed({ ...old, virtual_display_mode: 'per_client' }), true);
  assert.equal(virtualDisplayUsed(withHostGates({ virtual_display_mode: '' }, metadata(true))), true);
  // A host that doesn't say counts as able.
  assert.equal(virtualDisplayUsed(withHostGates({ virtual_display_mode: '' }, null)), true);
  assert.equal(virtualDisplayUsed(withHostGates({ virtual_display_mode: 'disabled' }, null)), false);
  // Both pages take the host's gates from the one helper; the home hides the row with it.
  for (const name of ['pages/Settings.svelte', 'pages/SettingsHome.svelte']) {
    assert.match(await source(name), /\$derived\(withHostGates\(currentValues\(described, edits, config\), (session|host)\.metadata\)\)/, name);
  }
  assert.match(await source('pages/SettingsHome.svelte'), /key !== 'virtual_display_layout' \|\| virtualDisplayUsed\(values\)/);
});

test('pages call the library by its navigation name, Games', async () => {
  for (const name of ['pages/Stream.svelte', 'pages/Overview.svelte', 'pages/Library.svelte', 'App.svelte']) {
    assert.doesNotMatch(await source(name), /Games &(amp;)? apps/, name);
  }
});

test('capture timing diagnostics sit in Debug switches with their own text, and stay off the home', () => {
  assert.deepEqual([...DIAGNOSTICS].sort(), ['capture_poll_interval_us', 'capture_predictive_poll', 'wgc_slot_aligned_publish']);
  for (const key of DIAGNOSTICS) {
    const setting = settings.get(key);
    assert.equal(setting.group, DEBUG_GROUP, key);
    assert.equal(settingsGroup(setting.category), 'advanced', key);
    // Not an always-on switch: the description stays the setting's own.
    assert.doesNotMatch(setting.description, /On for everyone/, key);
    assert.equal(MAIN_KEYS.includes(key), false, key);
  }
});
