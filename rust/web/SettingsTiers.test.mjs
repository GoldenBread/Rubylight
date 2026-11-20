// The tiers of All settings (src/lib/schema/tiers.ts): what each topic tab shows, what Advanced lists
// as Debug switches, and that nothing a user changed is hidden.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const lib = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { schema, settingGroups, tabLayout, topicTab, topicGroup, placeOf, placeText, movedAnchor, GAME_SOURCES } = lib;
const main = await server.ssrLoadModule('/src/lib/schema/main.ts');
const tiers = await server.ssrLoadModule('/src/lib/schema/tiers.ts');
const { askedFor, differsFromDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { results, settingHref, settingLink } = await server.ssrLoadModule('/src/lib/quick-switch.ts');
const { ALWAYS_ON, DIAGNOSTICS, MAIN_KEYS, DEBUG_GROUP, FOLDED_GROUPS, TRAILING_GROUPS } = main;
const { GAMES_PAGE_KEYS, tierOf } = tiers;

const byKey = new Map(schema.settings.map((setting) => [setting.key, setting]));
const topics = settingGroups.filter((tab) => tab.id !== 'advanced').map((tab) => tab.id);
const unchanged = () => false;
const layoutOf = (tab, input = {}) => tabLayout(tab, schema.settings.filter((setting) => !setting.partOf), { changed: unchanged, ...input });
const keysOf = (groups) => groups.flatMap((group) => group.settings.map((setting) => setting.key));

test('every key in the tier lists is a setting, and no key has two tiers', () => {
  const lists = { 'games-page': GAMES_PAGE_KEYS, 'always-on': ALWAYS_ON, diagnostic: DIAGNOSTICS, home: MAIN_KEYS };
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
  assert.deepEqual([GAMES_PAGE_KEYS.length, ALWAYS_ON.length, DIAGNOSTICS.length], [15, 20, 3]);
  // No expert tier: those settings are everyday ones in their topic tab.
  assert.equal('EXPERT_KEYS' in tiers, false);
  assert.equal(tierOf('amd_preanalysis'), 'everyday');
});

test('no topic tab starts with more than 40 settings outside its folded groups', () => {
  const folded = (group) => FOLDED_GROUPS.has(group.name) || TRAILING_GROUPS.has(group.name);
  for (const tab of topics) {
    const count = keysOf(layoutOf(tab).groups.filter((group) => !folded(group))).length;
    assert.ok(count <= 40, `${tab} shows ${count}`);
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
      // An always-on setting shows in its topic tab once turned off, when its dependants stop applying.
      assert.ok(tier === 'everyday' || tier === 'home' || tier === 'always-on', `${setting.key} depends on ${key}, which is ${tier}`);
      assert.equal(topicTab(byKey.get(key)), topicTab(setting), `${setting.key} depends on ${key} in another tab`);
    }
    // A hint that names a setting must not name one the tab does not show.
    for (const other of schema.settings) {
      if (['everyday', 'home', 'always-on'].includes(tierOf(other.key)) || other.key === setting.key) continue;
      assert.ok(!(setting.inactiveHint ?? '').includes(`“${other.label}”`), `${setting.key}: hint names ${other.key}`);
    }
  }
});

test('every setting but the Debug switches and launcher options shows in its topic tab, unchanged; Advanced has no Expert section', () => {
  for (const tab of topics) {
    const layout = layoutOf(tab);
    const expected = schema.settings.filter((setting) => !setting.partOf && topicTab(setting) === tab && ['home', 'everyday'].includes(tierOf(setting.key)));
    assert.deepEqual(keysOf(layout.groups).sort(), expected.map((setting) => setting.key).sort(), tab);
    assert.deepEqual(Object.keys(layout).sort(), ['gameSources', 'groups'], tab);
  }
  // Former expert settings sit in their own group with the others.
  const picture = layoutOf('picture');
  assert.ok(picture.groups.find((group) => group.name === 'AMD AMF').settings.some((setting) => setting.key === 'amd_preanalysis'));
  assert.ok(picture.groups.find((group) => group.name === 'Bitrate and network').settings.some((setting) => setting.key === 'bitrate_adaptation'));
  assert.ok(layoutOf('pc').groups.find((group) => group.name === 'Tray').settings.some((setting) => setting.key === 'hide_tray_controls'));
  const advanced = layoutOf('advanced');
  assert.equal('expertGroups' in advanced, false);
  assert.ok(keysOf(advanced.groups).includes('min_log_level'));
  assert.ok(!advanced.groups.some((group) => group.name.includes(' › ')));
  // Debug switches hold the always-on and diagnostic settings, and nothing else.
  assert.deepEqual(keysOf(advanced.groups.filter((group) => group.name === DEBUG_GROUP)).sort(), [...ALWAYS_ON, ...DIAGNOSTICS].sort());
  // A Debug switch that was turned off also shows where it came from.
  const warp = layoutOf('picture', { changed: (setting) => setting.key === 'frame_warp' });
  assert.ok(warp.groups.find((group) => group.name === 'Frame pacing')?.settings.some((setting) => setting.key === 'frame_warp'));
});

test('a setting its choices hide stays out of the tab even when changed', () => {
  const layout = layoutOf('picture', { changed: (setting) => setting.key === 'amd_preanalysis', visible: (setting) => setting.key !== 'amd_preanalysis' });
  assert.ok(!keysOf(layout.groups).includes('amd_preanalysis'));
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
  assert.deepEqual(placeOf(amf), { tab: 'picture', group: 'AMD AMF' });
  assert.equal(placeText(amf), 'Picture › AMD AMF');
  assert.equal(settingHref(amf), '/settings/picture?setting=amd_preanalysis');
  assert.equal(settingLink('amd_preanalysis'), '/settings/picture?setting=amd_preanalysis');
  assert.equal(settingHref(byKey.get('stream_preset')), '/settings/picture?setting=stream_preset');
  assert.equal(settingHref(byKey.get('steam_enabled')), '/settings/games?setting=steam_enabled');
  assert.equal(placeText(byKey.get('frame_warp')), 'Advanced › Debug switches');
  assert.equal(placeText(byKey.get('stream_preset')), 'Picture › Preset');
  const found = results(amf.label, { apps: [], clients: [] }).find((group) => group.id === 'settings');
  const entry = found.entries.find((candidate) => candidate.id === 'setting:amd_preanalysis');
  assert.match(entry.detail ?? '', /^Picture › AMD AMF$/);
  assert.match(entry.href, /^\/settings\/picture\?setting=/);
  // Anchors of the former Expert section lead back to the topic group; a group of Debug switches leads to them.
  assert.equal(movedAnchor('picture', 'amd-amf'), null);
  assert.deepEqual(movedAnchor('advanced', 'picture-amd-amf'), { tab: 'picture', anchor: 'amd-amf' });
  assert.deepEqual(movedAnchor('advanced', 'this-pc-updates'), { tab: 'pc', anchor: 'updates' });
  assert.deepEqual(movedAnchor('advanced', 'advanced-logging'), { tab: 'advanced', anchor: 'logging' });
  assert.equal(movedAnchor('advanced', 'logging'), null);
  assert.equal(movedAnchor('advanced', 'debug-switches'), null);
  assert.deepEqual(movedAnchor('picture', 'capture-troubleshooting'), { tab: 'advanced', anchor: 'debug-switches' });
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
    for (const key of keysOf(layout.groups)) shown.set(key, [...(shown.get(key) ?? []), tab.id]);
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
  const stored = { bitrate_adaptation: 'delay', frame_warp: 'disabled' };
  const advanced = layoutOf('advanced', { changed: asked(stored) });
  assert.ok(keysOf(advanced.groups).includes('frame_warp'));
  const picture = layoutOf('picture', { changed: asked(stored) });
  assert.ok(keysOf(picture.groups).includes('frame_warp'));
  assert.ok(!keysOf(layoutOf('picture', { changed: asked({}) }).groups).includes('frame_warp'));
  const edited = layoutOf('picture', { changed: asked({}, {}, new Set(['stream_preset'])) });
  assert.ok(keysOf(edited.groups).includes('stream_preset'));
});

test('a JSON setting stored as empty JSON is at its default', () => {
  const keybindings = byKey.get('keybindings');
  assert.equal(keybindings.control.kind, 'json');
  assert.equal(differsFromDefault(keybindings, {}, { keybindings: '[]' }), false);
  assert.equal(differsFromDefault(keybindings, {}, { keybindings: '[{"keys":"a"}]' }), true);
});

test('a changed launcher option shows in the Games tab', () => {
  const layout = layoutOf('games', { changed: (setting) => setting.key === 'epic_auto_sync' });
  assert.ok(keysOf(layout.groups).includes('epic_auto_sync'));
});

test('turning off QR pairing shows it beside Allow pairing; HDR dynamic metadata and the NVENC power choice are everyday', () => {
  const network = layoutOf('network', { changed: (setting) => setting.key === 'qr_pairing' });
  const pairing = network.groups.find((group) => group.name === 'Pairing');
  assert.ok(pairing.settings.some((setting) => setting.key === 'qr_pairing'));
  assert.ok(pairing.settings.some((setting) => setting.key === 'enable_pairing'));
  for (const key of ['hdr_dynamic_metadata', 'nvenc_latency_over_power']) assert.equal(tierOf(key), 'everyday', key);
});
