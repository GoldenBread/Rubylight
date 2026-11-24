// What the Settings home shows, and how All settings folds the rest
// (docs: unified/ux/direction.md 2.2-2.3). The tier/group tables are generated
// from rust/core/config/settings.toml into tiers.gen.ts.
import type { ConfigValue } from '../api';
import { SETTING } from '../stream-presets';
import { VIRTUAL_DISPLAY_OS, type Option, type Setting } from '../settings-types';
import { DEBUG_GROUP, ORIGIN_CATEGORY } from './tiers.gen';

export {
  ALWAYS_ON,
  DEBUG_GROUP,
  DIAGNOSTICS,
  FOLDED_GROUPS,
  MAIN_BUDGET,
  MAIN_KEYS,
  MAIN_SECTIONS,
  TRAILING_GROUPS,
  UPDATE_KEYS,
  type MainSection,
} from './tiers.gen';

/** An always-on setting as All settings shows it: in Debug switches, for troubleshooting. */
export function asDebugSwitch(setting: Setting): Setting {
  return {
    ...setting,
    origin: { category: ORIGIN_CATEGORY[setting.key] ?? setting.category, group: setting.group },
    category: 'advanced',
    group: DEBUG_GROUP,
    description: 'On for everyone. Turning it off is only for troubleshooting.',
    details: [setting.description, setting.details].filter(Boolean).join(' '),
    pending: undefined,
  };
}

/** A troubleshooting setting as All settings shows it: in Debug switches, as it is described. */
export function asDiagnostic(setting: Setting): Setting {
  return { ...setting, origin: { category: setting.category, group: setting.group }, category: 'advanced', group: DEBUG_GROUP };
}

/**
 * The monitor layout as an outcome: Turn off is Exclusive, Stay on is Extend.
 * Another layout chosen in All settings stays listed under its own name, so
 * the home never rewrites it.
 */
export function monitorsWhileStreaming(setting: Setting, current: ConfigValue): Setting {
  if (setting.control.kind !== 'select') return setting;
  const options: Option[] = [
    { value: 'exclusive', label: 'Turn off' },
    { value: 'extended', label: 'Stay on' },
  ];
  const other = setting.control.options.find(
    (option) => option.value === current && !options.some((item) => item.value === option.value),
  );
  if (other) options.push({ value: other.value, label: `Stay on: ${other.label}` });
  return {
    ...setting,
    label: 'While streaming, your monitors',
    description:
      'Turn off shows the game only on your device: your monitors go dark while you stream and come back when you stop. Stay on keeps them on, and games can open there instead of on the stream.',
    control: { kind: 'select', options },
  };
}

/**
 * The stream preset as the home shows it: the trade-off first, in words a
 * player uses, without pointing at encoder groups that are not on this page.
 */
export function streamPreset(setting: Setting): Setting {
  if (setting.control.kind !== 'select') return setting;
  return {
    ...setting,
    description:
      'The delay and picture quality of every stream in one choice. Balanced suits almost everyone; a setting you change in All settings still wins.',
    control: {
      kind: 'select',
      options: setting.control.options.map((option) =>
        option.value === 'balanced' ? { ...option, label: `${option.label} (recommended)` } : option,
      ),
    },
  };
}

/**
 * What a preset gets and costs, in one sentence for the home, from its id;
 * undefined for an id this console has no words for, so a new preset in the
 * shared table gets a test failure rather than another preset's sentence.
 */
export function presetOutcome(id: string): string | undefined {
  switch (id) {
    case 'lowest_latency':
      return 'Handles each frame as fast as it can. Some picture quality is lost.';
    case 'reference':
      return 'Removes the size limit on recovery frames, so a loss costs more bandwidth; no picture-quality gain from it has been measured. Use it only on a fast, steady link.';
    case 'balanced':
      return 'Rubylight’s defaults: the safest choice for almost every game.';
  }
  return undefined;
}

/** What the host says about its virtual display driver, as `/api/metadata` sends it. */
export interface VirtualDisplayDriver {
  capable: boolean;
  ready: boolean;
}

/**
 * Virtual display while the stored value is Default: say what this PC does,
 * since one per device and a monitor are both called Default by Windows
 * version. A driver that is installed but not ready, or missing, is not
 * promised as a screen. A chosen mode keeps the setting's own words.
 */
export function streamDisplay(
  setting: Setting,
  current: ConfigValue,
  values: Record<string, ConfigValue>,
  driver?: VirtualDisplayDriver,
): Setting {
  if (setting.key !== 'virtual_display_mode' || current !== '') return setting;
  const os = values[VIRTUAL_DISPLAY_OS] !== false;
  let description: string;
  if (!os) {
    description =
      'Streams show one of your monitors: this Windows cannot run Rubylight’s virtual display driver. Windows 11 23H2 or later can.';
  } else if (driver && !driver.capable) {
    description =
      'Streams show one of your monitors: Rubylight’s virtual display driver is not available on this PC. The checks on Home say what is missing.';
  } else if (driver && !driver.ready) {
    description =
      'Streams use a virtual display once the driver is ready; until then they show one of your monitors. The checks on Home say why it is not ready.';
  } else {
    description =
      'Each stream gets an extra screen at the device’s resolution and refresh rate instead of a monitor. By default, this PC makes one for each device; a device’s or app’s choice wins.';
  }
  return { ...setting, description };
}

/** "2560x1440" as people read a resolution; any other text as stored. */
function resolutionText(value: string): string {
  return /^\d+x\d+$/i.test(value) ? value.replace(/x/i, ' × ') : value;
}

/**
 * The stream resolution as the home shows it: the two choices that need no
 * other field, and a stored fixed value under its own name. Choosing a fixed
 * resolution is a field of its own in All settings, so the home can never
 * leave it empty and fail every stream.
 */
export function streamResolution(setting: Setting, current: ConfigValue, values: Record<string, ConfigValue>): Setting {
  if (setting.control.kind !== 'select') return setting;
  const options: Option[] = [
    { value: 'auto', label: 'Match the device' },
    { value: 'disabled', label: 'Keep the current resolution' },
  ];
  if (current === 'manual') {
    const fixed = typeof values.dd_manual_resolution === 'string' ? values.dd_manual_resolution.trim() : '';
    options.push({
      value: 'manual',
      label: fixed ? `Fixed: ${resolutionText(fixed)}` : 'Fixed resolution: set it in All settings',
    });
  }
  return {
    ...setting,
    label: 'Stream resolution',
    description:
      'The resolution each stream runs at. Matching the device suits almost everyone: a streamed monitor switches to it and back, while a virtual display follows the device unless a fixed resolution is set.',
    control: { kind: 'select', options },
  };
}

/** How a main key looks on the Settings home. */
export function onHome(
  setting: Setting,
  current: ConfigValue,
  values: Record<string, ConfigValue> = {},
  driver?: VirtualDisplayDriver,
): Setting {
  switch (setting.key) {
    case 'virtual_display_layout':
      return monitorsWhileStreaming(setting, current);
    case 'virtual_display_mode':
      return streamDisplay(setting, current, values, driver);
    case 'dd_resolution_option':
      return streamResolution(setting, current, values);
    case SETTING:
      return streamPreset(setting);
    default:
      return setting;
  }
}
