// The settings the Rust host reads, with its real defaults. Each part file
// covers some categories.
import type { Category, CategoryId, Schema, Setting } from './settings-types';
import { settings as basics } from './schema/basics';
import { settings as video } from './schema/video';
import { settings as display } from './schema/display';
import { settings as library } from './schema/library';
import { settings as extensions } from './schema/extensions';
import { ALWAYS_ON, DIAGNOSTICS, EXPERT_GROUPS, DEBUG_GROUP, asDebugSwitch, asDiagnostic } from './schema/main';
import { GAMES_PAGE_KEY_SET, tierOf } from './schema/tiers';

// Topics for search: a setting is found by its topic's name too. What people
// see is the settings tabs below, which gather these topics.
const categories: Category[] = [
  { id: 'general', label: 'This PC', description: 'Name, updates, sleep and the tray' },
  { id: 'network', label: 'Network', description: 'Discovery, encryption and sharing' },
  { id: 'video', label: 'Video', description: 'Capture, codecs, bitrate and frame pacing' },
  { id: 'encoders', label: 'Encoders', description: 'Which GPU and encoder compress the video' },
  { id: 'display', label: 'Display', description: 'Which screen streams and virtual displays' },
  { id: 'frame-limiting', label: 'Frame limiting', description: 'Frame rate caps for games' },
  { id: 'hdr', label: 'HDR', description: 'HDR on the streamed display and in the video' },
  { id: 'audio', label: 'Audio', description: 'Sound, speakers and microphone' },
  { id: 'input', label: 'Input', description: 'Keyboard, mouse, controllers and USB' },
  { id: 'commands', label: 'Commands', description: 'Commands run around apps or from a device' },
  { id: 'library', label: 'Games', description: 'Game sources, app behavior and co-op' },
  { id: 'advanced', label: 'Advanced', description: 'Sign-in, logs, compatibility and custom keys' },
];

const basicKeys = new Set([
  'sunshine_name',
  'enable_pairing',
  'system_tray',
  'hide_tray_controls',
  'update_check_interval',
  'update_check_time',
  'auto_update',
  'notify_pre_releases',
  'sleep_after_idle_minutes',
  'stream_preset',
  'max_bitrate',
  'frame_limiter_enable',
  'frame_limiter_fps_limit',
  'output_name',
  'virtual_display_mode',
  'virtual_display_layout',
  'dd_configuration_option',
  'dd_resolution_option',
  'dd_manual_resolution',
  'dd_refresh_rate_option',
  'dd_manual_refresh_rate',
  'dd_hdr_option',
  'stream_audio',
  'audio_capture_mode',
  'audio_sink',
  'stream_mic',
  'keyboard',
  'mouse',
  'controller',
  'gamepad',
  'forward_rumble',
  'terminate_on_pause',
  'enable_discovery',
  'internet_play',
  'webrtc_server',
  'file_transfer',
  'usb_forwarding',
  'origin_web_ui_allowed',
  'playnite_enabled',
  'playnite_auto_sync',
  'playnite_sync_categories',
  'playnite_fullscreen_entry_enabled',
  'steam_enabled',
  'steam_auto_sync',
]);

export const schema: Schema = {
  categories,
  // Always-on features keep their keys, shown only under Debug switches.
  settings: [...basics, ...video, ...display, ...library, ...extensions]
    .filter(setting => !setting.pending || ALWAYS_ON.includes(setting.key))
    .map(setting => (ALWAYS_ON.includes(setting.key) ? asDebugSwitch(setting) : setting))
    .map(setting => (DIAGNOSTICS.includes(setting.key) ? asDiagnostic(setting) : setting))
    .map(setting => ({ ...setting, advanced: !basicKeys.has(setting.key) })),
};

/** The All settings tabs, the same names and order as the sections of the Settings home. */
export type TabId = 'pc' | 'picture' | 'sound' | 'games' | 'network' | 'advanced';

export interface SettingsTab {
  id: TabId;
  label: string;
  description: string;
}

export const settingGroups: SettingsTab[] = [
  { id: 'pc', label: 'This PC', description: 'Name, updates, sleep and wake' },
  { id: 'picture', label: 'Picture', description: 'Quality, screens, HDR and frame rate' },
  { id: 'sound', label: 'Sound & controls', description: 'Sound, microphone, keyboard, mouse and controllers' },
  { id: 'games', label: 'Games', description: 'Game sources, app behavior and commands' },
  { id: 'network', label: 'Network', description: 'Discovery, encryption and sharing with devices' },
  { id: 'advanced', label: 'Advanced', description: 'Sign-in, logs, compatibility and custom keys' },
];

const TAB_IDS: ReadonlySet<string> = new Set(settingGroups.map((tab) => tab.id));

/**
 * The tab a category's settings show under. Also reads a tab's own id, and
 * the ids older links used (/settings/general, /settings/video, …), so they
 * keep opening the tab that now holds those settings.
 */
export function settingsGroup(id: string): TabId {
  if (TAB_IDS.has(id)) return id as TabId;
  switch (id) {
    case 'general': return 'pc';
    case 'video': case 'encoders': case 'frame-limiting': case 'display': case 'hdr': return 'picture';
    case 'audio': case 'input': return 'sound';
    case 'library': case 'commands': return 'games';
    case 'network': return 'network';
    case 'advanced': return 'advanced';
    default: return 'pc';
  }
}

/** The tab a /settings/:section link opens: its own, or the one an older id now means. */
export function tabOf(section: string): SettingsTab {
  return settingGroups.find((tab) => tab.id === settingsGroup(section)) ?? settingGroups[0]!;
}

/** The categories a setting shows under: its tab, then its own topic. */
export function categoriesOf(setting: { category: CategoryId }): { id: string; label: string; description: string }[] {
  return [
    settingGroups.find((group) => group.id === settingsGroup(setting.category)),
    categories.find((category) => category.id === setting.category),
  ].filter((category): category is SettingsTab | Category => !!category);
}

/**
 * The order of each tab's groups: the ones people look for first. Groups not
 * listed follow in the order the schema first uses them; the expert groups,
 * which start folded, come last, Debug switches at the very end.
 */
const GROUP_ORDER: Record<TabId, readonly string[]> = {
  pc: ['Name and language', 'Updates', 'Sleep and wake', 'Privacy', 'Tray', 'Hardware metrics'],
  picture: [
    'Preset', 'Streamed display', 'Resolution and refresh rate', 'HDR', 'RTX HDR', 'Codecs', 'Bitrate and network',
    'Frame pacing', 'Frame limiter', 'Encoder', 'RTX Video enhancement', 'Capture', 'After a stream',
    'Remote monitors',
  ],
  sound: [
    'Sound', 'Audio devices', 'Virtual speakers', 'Microphone', 'Input devices', 'Keyboard',
    'Mouse, pen and touch', 'Controllers', 'Steam Deck and back grips', 'USB devices',
  ],
  games: [
    'App behavior', 'Game sources', 'Steam options', 'Epic Games options', 'Battle.net options', 'Microsoft Store and Xbox options',
    'EA app options', 'Ubisoft Connect options', 'GOG options', 'Playnite', 'Playnite launch focus', 'Game folders',
    'Covers', 'Play together', 'Lossless Scaling', 'Commands around every app', 'Commands from devices',
  ],
  network: ['Pairing', 'Discovery and internet', 'Streams', 'WebRTC', 'File transfer', 'Shared folders', 'Clipboard'],
  advanced: ['Logging', 'Compatibility'],
};

/** Sort key of a group within its tab (see GROUP_ORDER); ties keep schema order. */
export function groupRank(tab: TabId, name: string): number {
  if (name === DEBUG_GROUP) return 3000;
  if (EXPERT_GROUPS.has(name)) return 2000;
  const at = GROUP_ORDER[tab].indexOf(name);
  return at === -1 ? 1000 : at;
}

/** The id of a group's section on its tab, for links such as /settings/pc#updates. */
export function groupAnchor(name: string): string {
  return (
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '') || 'settings'
  );
}

// ---------- where a setting shows: its tier (schema/tiers.ts) placed on the tabs ----------

/** The heading Advanced gives the expert settings, and the line under it. */
export const EXPERT_HEADING = 'Expert settings';
export const EXPERT_NOTE = 'Tuning and compatibility. The defaults are the measured ones; change these only for a reason.';
/** The row that stands in for the launchers' option groups in the Games tab. */
export const GAME_SOURCES = 'Game sources';

/** The tab a setting belongs to by topic: its own, or where a Debug switch came from. */
export const topicTab = (setting: Pick<Setting, 'category' | 'origin'>): TabId => settingsGroup((setting.origin ?? setting).category);
/** The group a setting has in its topic tab. */
export const topicGroup = (setting: Pick<Setting, 'group' | 'origin'>): string => (setting.origin ? setting.origin.group : setting.group) ?? '';

const tabLabel = (id: TabId): string => settingGroups.find((tab) => tab.id === id)?.label ?? id;

/** The name of an expert setting's group in Advanced: "Picture › AMD AMF". */
export function expertGroupName(setting: Pick<Setting, 'category' | 'group' | 'origin'>): string {
  const group = topicGroup(setting);
  const label = tabLabel(topicTab(setting));
  return group ? `${label} › ${group}` : label;
}

/** Where a setting is found by default: the tab and group it shows in while it is not changed. */
export function placeOf(setting: Setting): { tab: TabId; group: string; expert: boolean } {
  switch (tierOf(setting.key)) {
    case 'expert':
      return { tab: 'advanced', group: expertGroupName(setting), expert: true };
    case 'always-on':
    case 'diagnostic':
      return { tab: 'advanced', group: DEBUG_GROUP, expert: false };
    default:
      return { tab: topicTab(setting), group: topicGroup(setting), expert: false };
  }
}

/** Where a setting is, as one line: "Advanced › Expert settings › Picture › AMD AMF". */
export function placeText(setting: Setting): string {
  const place = placeOf(setting);
  const path = place.expert ? [tabLabel(place.tab), EXPERT_HEADING, place.group] : [tabLabel(place.tab), place.group];
  return path.filter(Boolean).join(' › ');
}

export interface LayoutGroup {
  name: string;
  settings: Setting[];
}

export interface ExpertGroup extends LayoutGroup {
  /** The topic tab the group comes from. */
  tab: TabId;
  rank: number;
}

export interface TabLayout {
  /** The groups of the tab itself, in page order. */
  groups: LayoutGroup[];
  /** Advanced only: the expert settings, one collapsed group per topic group, in tab order. */
  expertGroups: ExpertGroup[];
  /** Settings shown in a topic tab that are not everyday: they carry the Expert mark. */
  marked: ReadonlySet<string>;
  /** The Games tab: the launchers' groups are one link to the Games page. */
  gameSources: boolean;
  /** A topic tab: its expert settings not shown here, and the Advanced group the first of them is in. */
  expertCount: number;
  expertTarget: string | undefined;
}

export interface LayoutInput {
  /** Changed from the default, saved or not: such a setting always shows in its topic tab. */
  changed: (setting: Setting) => boolean;
  /** Whether the current choices make a setting apply; the ones that do not stay out of the page. */
  visible?: (setting: Setting) => boolean;
  /** Launcher groups to list in full (opened by a link to the group or to one of its settings). */
  revealed?: ReadonlySet<string>;
}

function grouped(list: readonly Setting[], tab: TabId, name: (setting: Setting) => string): LayoutGroup[] {
  const groups: LayoutGroup[] = [];
  for (const setting of list) {
    const key = name(setting);
    let group = groups.find((candidate) => candidate.name === key);
    if (!group) groups.push((group = { name: key, settings: [] }));
    group.settings.push(setting);
  }
  return groups.sort((a, b) => groupRank(tab, a.name) - groupRank(tab, b.name));
}

/** Every expert group, in Advanced's order: the tabs in order, then each tab's own group order. */
function expertGroupsOf(settings: readonly Setting[], visible: (setting: Setting) => boolean): ExpertGroup[] {
  const found = new Map<string, ExpertGroup>();
  for (const setting of settings) {
    if (tierOf(setting.key) !== 'expert' || !visible(setting)) continue;
    const name = expertGroupName(setting);
    let group = found.get(name);
    if (!group) {
      const tab = topicTab(setting);
      found.set(name, (group = { name, settings: [], tab, rank: groupRank(tab, topicGroup(setting)) }));
    }
    group.settings.push(setting);
  }
  const order = (tab: TabId) => settingGroups.findIndex((candidate) => candidate.id === tab);
  return [...found.values()].sort((a, b) => order(a.tab) - order(b.tab) || a.rank - b.rank);
}

/**
 * What a tab shows. A topic tab shows its everyday settings, and any other
 * setting of its topic that has been changed, so nothing a user changed is
 * hidden. Advanced shows the expert settings, collapsed by topic group, and
 * the Debug switches. `settings` are the described ones without those edited
 * inside another row (partOf).
 */
export function tabLayout(tab: TabId, settings: readonly Setting[], input: LayoutInput): TabLayout {
  const visible = input.visible ?? (() => true);
  const revealed = input.revealed ?? new Set<string>();
  const marked = new Set<string>();
  const experts = expertGroupsOf(settings, visible);
  if (tab === 'advanced') {
    const own = settings.filter((setting) => {
      const tier = tierOf(setting.key);
      const native = topicTab(setting) === 'advanced' && (tier === 'everyday' || tier === 'home');
      return visible(setting) && (tier === 'always-on' || tier === 'diagnostic' || native);
    });
    return { groups: grouped(own, tab, (setting) => setting.group ?? ''), expertGroups: experts, marked, gameSources: false, expertCount: 0, expertTarget: undefined };
  }
  const inTopic = settings.filter((setting) => topicTab(setting) === tab);
  const shown = inTopic.filter((setting) => {
    if (!visible(setting)) return false;
    const tier = tierOf(setting.key);
    if (tier === 'home' || tier === 'everyday') return true;
    if (input.changed(setting)) {
      marked.add(setting.key);
      return true;
    }
    return tier === 'games-page' && revealed.has(topicGroup(setting));
  });
  const groups = grouped(shown, tab, topicGroup);
  const sources = tab === 'games' && inTopic.some((setting) => GAMES_PAGE_KEY_SET.has(setting.key) && !revealed.has(topicGroup(setting)));
  if (sources) {
    groups.push({ name: GAME_SOURCES, settings: [] });
    groups.sort((a, b) => groupRank(tab, a.name) - groupRank(tab, b.name));
  }
  const hidden = inTopic.filter((setting) => tierOf(setting.key) === 'expert' && visible(setting) && !input.changed(setting));
  return {
    groups,
    expertGroups: [],
    marked,
    gameSources: sources,
    expertCount: hidden.length,
    expertTarget: hidden.length ? experts.find((group) => group.tab === tab)?.name : undefined,
  };
}

/**
 * Where an older link to a group (/settings/picture#amd-amf) now leads: the
 * expert group in Advanced, or the Debug switches. Null while the group still
 * has everyday settings in that tab, or is a launcher group the tab can open.
 */
export function movedAnchor(tab: TabId, anchor: string): { tab: TabId; anchor: string } | null {
  const inGroup = schema.settings.filter((setting) => topicTab(setting) === tab && groupAnchor(topicGroup(setting) || tabLabel(tab)) === anchor);
  if (inGroup.length === 0 || inGroup.some((setting) => ['home', 'everyday', 'games-page'].includes(tierOf(setting.key)))) return null;
  const expert = inGroup.find((setting) => tierOf(setting.key) === 'expert');
  if (expert) return { tab: 'advanced', anchor: groupAnchor(expertGroupName(expert)) };
  return { tab: 'advanced', anchor: groupAnchor(DEBUG_GROUP) };
}
