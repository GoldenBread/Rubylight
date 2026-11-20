// What the Settings home shows, and how All settings folds the rest
// (docs: unified/ux/direction.md 2.2-2.3). The keys are the schema's own;
// this file only chooses and orders them.
import type { ConfigValue } from '../api';
import { SETTING } from '../stream-presets';
import { VIRTUAL_DISPLAY_OS, type Option, type Setting } from '../settings-types';

export interface MainSection {
  id: string;
  title: string;
  description: string;
  /** Schema keys in the order they show. */
  keys: readonly string[];
}

/**
 * The Settings home: what a typical player changes in week one, phrased as
 * an outcome, with every option safe. At most 12 keys in all. Its sections
 * are the first All settings tabs, with the same ids and names
 * (settings-schema.ts settingGroups), so each links to the rest of its topic.
 */
export const MAIN_SECTIONS: readonly MainSection[] = [
  {
    id: 'pc',
    title: 'This PC',
    description: 'Its name, waking it from a device, and updates.',
    // Install updates automatically shows in the Updates panel, above the release status.
    keys: ['sunshine_name', 'auto_update'],
  },
  {
    id: 'picture',
    title: 'Picture',
    description: 'Stream quality, resolution and what happens to your screens while you play.',
    keys: ['stream_preset', 'virtual_display_mode', 'dd_resolution_option', 'virtual_display_layout'],
  },
  {
    id: 'sound',
    title: 'Sound & controls',
    description: 'What your devices can hear and use on this PC.',
    keys: ['stream_audio', 'stream_mic', 'controller'],
  },
  {
    id: 'games',
    title: 'Games',
    description: 'What happens to a game when you stop playing.',
    keys: ['terminate_on_pause'],
  },
];

/** The main keys the home shows in its Updates panel rather than among its section's rows. */
export const UPDATE_KEYS: readonly string[] = ['auto_update'];

export const MAIN_KEYS: readonly string[] = MAIN_SECTIONS.flatMap((section) => section.keys);
export const MAIN_BUDGET = 12;

/**
 * Right for everyone, no added latency, no PC side effect, stock Moonlight
 * unaffected and verified: on with no switch of their own. Their keys stay
 * readable and show only in All settings, Advanced, Debug switches (and in
 * their topic tab once turned off, like any changed setting).
 */
export const ALWAYS_ON: readonly string[] = [
  // Network
  'qr_pairing', 'client_roaming', 'bandwidth_probe',
  // Sound & controls
  'audio_passthrough', 'audio_surround_714',
  // Picture: capture, codecs, HDR, frame pacing, bitrate and NVENC
  'idd_push_fence_ring', 'wgc_direct_encoder_input', 'gpu_compute_conversion',
  'packed_444', 'pyrowave',
  'hdr_dolby_vision', 'hdr_hlg',
  'wgc_pacing_smoothing', 'stream_reconfigure', 'frame_warp',
  'wireless_adaptive_pacing', 'keyframe_loss_pacing',
  'nvenc_frame_budget_guard', 'nvenc_realtime_hags',
];
/**
 * Settings only for troubleshooting, whose values are not choices a player
 * makes: shown only in All settings, Advanced, Debug switches, with their own
 * descriptions (and in their topic tab once changed).
 */
export const DIAGNOSTICS: readonly string[] = ['capture_poll_interval_us', 'capture_predictive_poll', 'wgc_slot_aligned_publish'];
export const DEBUG_GROUP = 'Debug switches';
/** Where a Debug switch shows once turned off, when that is not the category it is filed under: QR pairing sits beside Allow pairing. */
const ORIGIN_CATEGORY: Readonly<Record<string, Setting['category']>> = { qr_pairing: 'network' };

/** The groups All settings starts collapsed and lists last in their tab. */
export const TRAILING_GROUPS: ReadonlySet<string> = new Set([
  // Settings written as JSON, each folded under its own name.
  'Mode substitutions',
  'Key remapping',
  'Capture troubleshooting',
  'AMD AMF',
  'NVIDIA NVENC',
  'RTSS',
  'Ports and addresses',
  'Console',
  DEBUG_GROUP,
]);

/**
 * Groups All settings also starts folded, in their usual place: the ones after
 * the everyday rows that most people leave alone. A customized group starts
 * open, and a link to one of its settings opens it.
 *
 * The launcher groups are here because the Games page already carries each
 * launcher's switch and its own "Options" link. Capture and Remote monitors
 * say themselves that they are for troubleshooting or an extra screen, and
 * Steam Deck and back grips is hardware tuning in an everyday tab. Bitrate and
 * network, Keyboard, Mouse, pen and touch and Hardware metrics are mostly
 * tuning whose defaults are the measured ones.
 */
export const FOLDED_GROUPS: ReadonlySet<string> = new Set([
  'HDR',
  'Frame pacing',
  'Frame limiter',
  'After a stream',
  'Capture',
  'Remote monitors',
  'Steam options',
  'Epic Games options',
  'Battle.net options',
  'Microsoft Store and Xbox options',
  'EA app options',
  'Ubisoft Connect options',
  'GOG options',
  'Playnite',
  'Playnite launch focus',
  'Game folders',
  'Lossless Scaling',
  'Steam Deck and back grips',
  'Bitrate and network',
  'Keyboard',
  'Mouse, pen and touch',
  'Hardware metrics',
]);

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
