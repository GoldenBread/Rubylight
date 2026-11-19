// The tiers of All settings (src/lib/schema/tiers.ts): what each topic tab shows, what Advanced lists
// as expert settings, and that nothing a user changed is hidden.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const lib = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { schema, settingGroups, tabLayout, topicTab, topicGroup, placeOf, placeText, movedAnchor, groupAnchor, EXPERT_HEADING, GAME_SOURCES } = lib;
const main = await server.ssrLoadModule('/src/lib/schema/main.ts');
const tiers = await server.ssrLoadModule('/src/lib/schema/tiers.ts');
const { askedFor, differsFromDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { results, settingHref, settingLink } = await server.ssrLoadModule('/src/lib/quick-switch.ts');
const { ALWAYS_ON, DIAGNOSTICS, MAIN_KEYS, DEBUG_GROUP } = main;
const { EXPERT_KEYS, GAMES_PAGE_KEYS, tierOf } = tiers;

const byKey = new Map(schema.settings.map((setting) => [setting.key, setting]));
const topics = settingGroups.filter((tab) => tab.id !== 'advanced').map((tab) => tab.id);
const unchanged = () => false;
const layoutOf = (tab, input = {}) => tabLayout(tab, schema.settings.filter((setting) => !setting.partOf), { changed: unchanged, ...input });
const keysOf = (groups) => groups.flatMap((group) => group.settings.map((setting) => setting.key));

test('every key in the tier lists is a setting, and no key has two tiers', () => {
  const lists = { expert: EXPERT_KEYS, 'games-page': GAMES_PAGE_KEYS, 'always-on': ALWAYS_ON, diagnostic: DIAGNOSTICS, home: MAIN_KEYS };
  const seen = new Map();
  for (const [name, keys] of Object.entries(lists)) {
    assert.equal(new Set(keys).size, keys.length, `${name} lists a key twice`);
    for (const key of keys) {
      assert.ok(byKey.has(key), `${name}: ${key} is not in the schema`);
      assert.equal(seen.get(key), undefined, `${key} is in ${seen.get(key)} and ${name}`);
      seen.set(key, name);
      assert.equal(tierOf(key), name, key);
    }
  }
  assert.deepEqual([EXPERT_KEYS.length, GAMES_PAGE_KEYS.length, ALWAYS_ON.length, DIAGNOSTICS.length], [141, 35, 20, 3]);
});

test('no topic tab starts with more than 40 everyday settings', () => {
  for (const tab of topics) {
    const layout = layoutOf(tab);
    const count = keysOf(layout.groups).length;
    assert.ok(count <= 40, `${tab} shows ${count}`);
    assert.equal(layout.marked.size, 0, tab);
  }
});

test('an everyday setting depends only on settings shown with it', () => {
  const home = new Set(MAIN_KEYS);
  const reads = (setting) => {
    const found = new Set();
    for (const answer of [undefined, true, false, 'x']) {
      const values = new Proxy({}, {
        get: (_, key) => (typeof key === 'string' ? (found.add(key), answer) : undefined),
        has: (_, key) => (typeof key === 'string' && found.add(key) !== null),
      });
      try { setting.visibleWhen?.(values); } catch { /* a probe value the rule does not take */ }
    }
    return [...found].filter((key) => byKey.has(key));
  };
  for (const setting of schema.settings) {
    if (!home.has(setting.key) && tierOf(setting.key) !== 'everyday') continue;
    const needs = [...reads(setting), setting.partOf, setting.control.daily?.key].filter(Boolean);
    for (const key of needs) {
      const tier = tierOf(key);
      assert.ok(tier === 'everyday' || tier === 'home', `${setting.key} depends on ${key}, which is ${tier}`);
      assert.equal(topicTab(byKey.get(key)), topicTab(setting), `${setting.key} depends on ${key} in another tab`);
    }
    // A hint that names a setting must not name one the tab does not show.
    for (const other of schema.settings) {
      if (tierOf(other.key) === 'everyday' || tierOf(other.key) === 'home' || other.key === setting.key) continue;
      assert.ok(!(setting.inactiveHint ?? '').includes(`“${other.label}”`), `${setting.key}: hint names ${other.key}`);
    }
  }
});

test('an unchanged expert setting is not in its topic tab; Advanced lists it in a group named for the tab', () => {
  const advanced = layoutOf('advanced');
  const listed = keysOf(advanced.expertGroups);
  assert.deepEqual([...listed].sort(), [...EXPERT_KEYS].filter((key) => !byKey.get(key).partOf).sort());
  for (const tab of topics) assert.ok(!keysOf(layoutOf(tab).groups).some((key) => tierOf(key) === 'expert'), tab);
  const names = advanced.expertGroups.map((group) => group.name);
  assert.ok(names.includes('Picture › AMD AMF'));
  assert.ok(names.includes('Advanced › Logging'));
  // Tab order first, then the order the tab had.
  const order = settingGroups.map((tab) => tab.label);
  const tabOfName = (name) => order.findIndex((label) => name.startsWith(`${label} › `) || name === label);
  assert.deepEqual(names.map(tabOfName), [...names.map(tabOfName)].sort((a, b) => a - b));
  assert.ok(names.indexOf('Picture › Capture') < names.indexOf('Picture › AMD AMF'));
  assert.ok(names.indexOf('This PC › Updates') < names.indexOf('Picture › Capture'));
  // Debug switches hold the always-on and diagnostic settings, and nothing else.
  assert.deepEqual(keysOf(advanced.groups.filter((group) => group.name === DEBUG_GROUP)).sort(), [...ALWAYS_ON, ...DIAGNOSTICS].sort());
});

test('a changed expert setting also shows in its topic tab, in its usual group, with the Expert mark', () => {
  for (const [key, tab, group] of [['amd_preanalysis', 'picture', 'AMD AMF'], ['congestion_control', 'picture', 'Bitrate and network'], ['hide_tray_controls', 'pc', 'Tray']]) {
    const changed = (setting) => setting.key === key;
    const layout = layoutOf(tab, { changed });
    const shown = layout.groups.find((candidate) => candidate.name === group);
    assert.deepEqual(shown?.settings.filter((setting) => setting.key === key).map((setting) => setting.key), [key], key);
    assert.ok(layout.marked.has(key), key);
    // Only that one: the rest of its expert group stays in Advanced.
    if (group === 'AMD AMF') assert.deepEqual(shown.settings.map((setting) => setting.key), [key]);
    // Advanced still lists it, and the link no longer counts it.
    assert.ok(keysOf(layoutOf('advanced', { changed }).expertGroups).includes(key), key);
    assert.equal(layout.expertCount, layoutOf(tab).expertCount - 1, key);
  }
  // A Debug switch that was turned off shows where it came from.
  const warp = layoutOf('picture', { changed: (setting) => setting.key === 'frame_warp' });
  assert.deepEqual(warp.groups.find((group) => group.name === 'Frame pacing')?.settings.map((setting) => setting.key), ['frame_warp']);
  assert.ok(warp.marked.has('frame_warp'));
});

test('a setting its choices hide stays out of the tab even when changed', () => {
  const layout = layoutOf('picture', { changed: (setting) => setting.key === 'amd_preanalysis', visible: (setting) => setting.key !== 'amd_preanalysis' });
  assert.ok(!keysOf(layout.groups).includes('amd_preanalysis'));
});

test('each topic tab ends with a link to its first expert group, counting the expert settings not shown', () => {
  const advanced = layoutOf('advanced');
  for (const tab of topics) {
    const layout = layoutOf(tab);
    const expected = schema.settings.filter((setting) => tierOf(setting.key) === 'expert' && topicTab(setting) === tab);
    assert.equal(layout.expertCount, expected.length, tab);
    assert.ok(layout.expertCount > 0, tab);
    const first = advanced.expertGroups.find((group) => group.name.startsWith(`${settingGroups.find((item) => item.id === tab).label} › `));
    assert.equal(layout.expertTarget, first.name, tab);
    assert.ok(advanced.expertGroups.some((group) => groupAnchor(group.name) === groupAnchor(layout.expertTarget)), tab);
  }
  assert.equal(layoutOf('picture').expertTarget, 'Picture › Streamed display');
  assert.equal(layoutOf('advanced').expertCount, 0);
  assert.equal(layoutOf('advanced').expertTarget, undefined);
});

test('the Games tab lists the launchers as one row to the Games page unless asked for them', () => {
  const games = layoutOf('games');
  assert.equal(games.gameSources, true);
  assert.ok(games.groups.some((group) => group.name === GAME_SOURCES && group.settings.length === 0));
  assert.ok(!keysOf(games.groups).some((key) => tierOf(key) === 'games-page'));
  assert.ok(!games.groups.some((group) => group.name === 'Steam options'));
  // The Game sources row sits after App behavior, before the rest.
  const names = games.groups.map((group) => group.name);
  assert.ok(names.indexOf('App behavior') < names.indexOf(GAME_SOURCES));
  assert.ok(names.indexOf(GAME_SOURCES) < names.indexOf('Game folders'));
  // A link to a launcher's group lists it in full; a changed option shows by itself.
  const opened = layoutOf('games', { revealed: new Set(['Steam options']) });
  assert.deepEqual(opened.groups.find((group) => group.name === 'Steam options').settings.length, byKeyCount('Steam options'));
  assert.equal(opened.gameSources, true);
  const changed = layoutOf('games', { changed: (setting) => setting.key === 'epic_auto_sync' });
  assert.deepEqual(changed.groups.find((group) => group.name === 'Epic Games options').settings.map((setting) => setting.key), ['epic_auto_sync']);
});

function byKeyCount(group) {
  return schema.settings.filter((setting) => setting.group === group).length;
}

test('a setting is found where it shows: links, older anchors and the quick switcher', () => {
  const amf = byKey.get('amd_preanalysis');
  assert.deepEqual(placeOf(amf), { tab: 'advanced', group: 'Picture › AMD AMF', expert: true });
  assert.equal(placeText(amf), `Advanced › ${EXPERT_HEADING} › Picture › AMD AMF`);
  assert.equal(settingHref(amf), '/settings/advanced?setting=amd_preanalysis');
  assert.equal(settingLink('amd_preanalysis'), '/settings/advanced?setting=amd_preanalysis');
  assert.equal(settingHref(byKey.get('stream_preset')), '/settings/picture?setting=stream_preset');
  assert.equal(settingHref(byKey.get('steam_auto_sync')), '/settings/games?setting=steam_auto_sync');
  assert.equal(placeText(byKey.get('frame_warp')), 'Advanced › Debug switches');
  assert.equal(placeText(byKey.get('stream_preset')), 'Picture › Preset');
  const found = results(amf.label, { apps: [], clients: [] }).find((group) => group.id === 'settings');
  const entry = found.entries.find((candidate) => candidate.id === 'setting:amd_preanalysis');
  assert.match(entry.detail ?? '', /^Advanced › Expert settings › Picture › AMD AMF$/);
  assert.match(entry.href, /^\/settings\/advanced\?setting=/);
  // Older anchors follow their group to Advanced; groups that stay put are left alone.
  assert.deepEqual(movedAnchor('picture', 'amd-amf'), { tab: 'advanced', anchor: 'picture-amd-amf' });
  assert.deepEqual(movedAnchor('picture', 'capture-troubleshooting'), { tab: 'advanced', anchor: 'debug-switches' });
  assert.deepEqual(movedAnchor('advanced', 'logging'), { tab: 'advanced', anchor: 'advanced-logging' });
  assert.equal(movedAnchor('picture', 'hdr'), null);
  assert.equal(movedAnchor('games', 'steam-options'), null);
  assert.equal(movedAnchor('games', 'app-behavior'), null);
  assert.equal(movedAnchor('picture', 'no-such-group'), null);
  // Every group name an anchor can name is in one place only.
  for (const setting of schema.settings) assert.ok(topicGroup(setting) !== undefined, setting.key);
});

test('every setting is in a topic tab or in Advanced, exactly where its tier says', () => {
  const shown = new Map();
  for (const tab of settingGroups) {
    const layout = layoutOf(tab.id);
    for (const key of [...keysOf(layout.groups), ...keysOf(layout.expertGroups)]) shown.set(key, [...(shown.get(key) ?? []), tab.id]);
  }
  for (const setting of schema.settings.filter((item) => !item.partOf)) {
    const tier = tierOf(setting.key);
    const tabs = shown.get(setting.key) ?? [];
    if (tier === 'games-page') assert.deepEqual(tabs, [], `${setting.key} stays on the Games page until asked for`);
    else assert.deepEqual(tabs, [placeOf(setting).tab], setting.key);
  }
});

test('the page decides "changed" with askedFor: stored values, unsaved edits, and nothing at the default', async () => {
  const page = await readFile(new URL('./src/pages/Settings.svelte', import.meta.url), 'utf8');
  assert.match(page, /askedFor\(setting, edits, config, pending\)/);
  const asked = (stored, edits = {}, pending = new Set()) => (setting) => askedFor(setting, edits, stored, pending);
  const stored = { congestion_control: 'enabled', frame_warp: 'disabled' };
  const advanced = layoutOf('advanced', { changed: asked(stored) });
  assert.ok(advanced.marked.has('frame_warp') || keysOf(advanced.groups).includes('frame_warp'));
  const picture = layoutOf('picture', { changed: asked(stored) });
  assert.ok(picture.marked.has('congestion_control') || keysOf(picture.groups).includes('congestion_control'));
  assert.equal(layoutOf('picture', { changed: asked({}) }).marked.size, 0);
  const edited = layoutOf('picture', { changed: asked({}, {}, new Set(['stream_preset'])) });
  assert.ok(keysOf(edited.groups).includes('stream_preset'));
});

test('a JSON setting stored as empty JSON is at its default', () => {
  const keybindings = byKey.get('keybindings');
  assert.equal(keybindings.control.kind, 'json');
  assert.equal(differsFromDefault(keybindings, {}, { keybindings: '[]' }), false);
  assert.equal(differsFromDefault(keybindings, {}, { keybindings: '[{"keys":"a"}]' }), true);
});

test('a changed launcher option is an ordinary option of the Games page, without the Expert mark', () => {
  const layout = layoutOf('games', { changed: (setting) => setting.key === 'epic_auto_sync' });
  assert.ok(keysOf(layout.groups).includes('epic_auto_sync'));
  assert.equal(layout.marked.has('epic_auto_sync'), false);
});

test('turning off QR pairing shows it beside Allow pairing; HDR dynamic metadata and the NVENC power choice are expert', () => {
  const network = layoutOf('network', { changed: (setting) => setting.key === 'qr_pairing' });
  const pairing = network.groups.find((group) => group.name === 'Pairing');
  assert.ok(pairing.settings.some((setting) => setting.key === 'qr_pairing'));
  assert.ok(pairing.settings.some((setting) => setting.key === 'enable_pairing'));
  assert.ok(network.marked.has('qr_pairing'));
  for (const key of ['hdr_dynamic_metadata', 'nvenc_latency_over_power']) assert.equal(tierOf(key), 'expert', key);
});
