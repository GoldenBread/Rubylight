// Which tier each setting belongs to (the host console's All settings):
//
//   home         the Settings home (MAIN_SECTIONS in main.ts)
//   everyday     shown in its topic tab: every key not listed anywhere
//   always-on    ALWAYS_ON in main.ts: on for everyone, in Advanced > Debug switches
//   diagnostic   DIAGNOSTICS in main.ts: troubleshooting, also in Debug switches
//   games-page   GAMES_PAGE_KEYS below: a launcher's options, which the Games page links to
//
// A setting whose value differs from its default also shows in its topic tab
// (settings-schema.ts tabLayout), so nothing a user changed is hidden. The
// comment above each run of keys names the tab and group the keys come from.
import { ALWAYS_ON, DIAGNOSTICS, MAIN_KEYS } from './main';

/** The launchers' option groups: the Games page links to each, so the Games tab keeps them out of the way. */
export const GAMES_PAGE_KEYS: readonly string[] = [
  // Games › Steam options
  'steam_enabled', 'steam_include_tools', 'steam_exclude_games',
  // Games › Epic Games options
  'library_epic_enabled', 'epic_exclude_games',
  // Games › Battle.net options
  'library_battlenet_enabled', 'battlenet_exclude_games',
  // Games › Microsoft Store and Xbox options
  'library_microsoft_enabled', 'microsoft_exclude_games',
  // Games › EA app options
  'library_ea_enabled', 'ea_exclude_games',
  // Games › Ubisoft Connect options
  'library_ubisoft_enabled', 'ubisoft_exclude_games',
  // Games › GOG options
  'library_gog_enabled', 'gog_exclude_games',
];

export const GAMES_PAGE_KEY_SET: ReadonlySet<string> = new Set(GAMES_PAGE_KEYS);
const HOME_KEY_SET: ReadonlySet<string> = new Set(MAIN_KEYS);

export type Tier = 'home' | 'everyday' | 'always-on' | 'diagnostic' | 'games-page';

/** The tier of a setting by its key. */
export function tierOf(key: string): Tier {
  if (HOME_KEY_SET.has(key)) return 'home';
  if (ALWAYS_ON.includes(key)) return 'always-on';
  if (DIAGNOSTICS.includes(key)) return 'diagnostic';
  if (GAMES_PAGE_KEY_SET.has(key)) return 'games-page';
  return 'everyday';
}

/** Shown in its topic tab whatever its value (the other tiers only once changed, or a launcher group when asked for). */
export const isEveryday = (key: string): boolean => {
  const tier = tierOf(key);
  return tier === 'home' || tier === 'everyday';
};
