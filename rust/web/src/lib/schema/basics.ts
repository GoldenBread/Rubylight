// Settings for the general, network, audio, input and commands categories.
// Generated from rust/core/config/settings.toml by tools/generated_docs.py.
import type { Setting } from '../settings-types';
import { settings as core } from './basics_core.gen';
import { settings as io } from './basics_io.gen';

export { on } from './rules.gen';

export const settings: Setting[] = [...core, ...io];
