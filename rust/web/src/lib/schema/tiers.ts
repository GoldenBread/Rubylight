// Which tier each setting belongs to (the host console's All settings).
// Tier lists are generated from rust/core/config/settings.toml into tiers.gen.ts.
import { ALWAYS_ON, DIAGNOSTICS, GAMES_PAGE_KEY_SET, MAIN_KEYS } from './tiers.gen';

export { GAMES_PAGE_KEYS, GAMES_PAGE_KEY_SET } from './tiers.gen';

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
