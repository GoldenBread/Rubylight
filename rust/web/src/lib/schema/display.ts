// Settings for display, frame limiting and HDR.
// Generated from rust/core/config/settings.toml by tools/generated_docs.py.
import type { Setting } from '../settings-types';
import { settings as screen } from './display_screen.gen';
import { settings as extras } from './display_extras.gen';

export const settings: Setting[] = [...screen, ...extras];
