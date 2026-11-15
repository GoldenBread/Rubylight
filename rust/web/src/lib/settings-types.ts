// The shape of the settings schema. The data lives in settings-schema.ts.
import type { BrowseType, ConfigValue } from './api';

export type CategoryId =
  | 'general'
  | 'network'
  | 'video'
  | 'encoders'
  | 'display'
  | 'frame-limiting'
  | 'hdr'
  | 'audio'
  | 'input'
  | 'commands'
  | 'library'
  | 'advanced';

export interface Category {
  id: CategoryId;
  label: string;
  description: string;
}

export interface Option {
  value: string;
  label: string;
}

export interface NumberPreset {
  value: number;
  label: string;
}

export type Control =
  | { kind: 'toggle' }
  | { kind: 'select'; options: Option[] }
  /**
   * `presets`: named values in a menu, with Custom for any other value in `unit`.
   * `daily` adds `label` to that menu: a time of day kept in the setting `key`
   * (a `time` text setting that is `partOf` this one) replaces the number.
   */
  | {
      kind: 'number';
      min?: number;
      max?: number;
      step?: number;
      unit?: string;
      presets?: NumberPreset[];
      daily?: { key: string; label: string };
    }
  /**
   * `browse` adds a picker for a program, file or folder on the host. `time`
   * makes it the browser's time field, for a time of day stored as HH:MM.
   * `presets`: named values in a menu, with Custom for any other text.
   */
  | { kind: 'text'; placeholder?: string; mono?: boolean; browse?: BrowseType; time?: boolean; presets?: Option[] }
  /** A string list, stored as a JSON array of strings. */
  | { kind: 'list'; placeholder?: string }
  /** Any JSON value, edited as text and validated. */
  | { kind: 'json' }
  /** Global preparation or state commands: [{do, undo, elevated}]. */
  | { kind: 'commands' }
  /** Commands clients may run: [{name, cmd, elevated}]. */
  | { kind: 'server-commands' }
  /** Shared folders: [{id, name, path}], an id given to each new folder from its name. */
  | { kind: 'folders' }
  /** A display, chosen from the host's displays (empty = automatic). */
  | { kind: 'display' }
  /** An audio endpoint, chosen from the host's endpoints (empty = default). */
  | { kind: 'audio' }
  /** A GPU, chosen by name from the host's adapters (empty = automatic). */
  | { kind: 'adapter' };

/**
 * Other spellings the host reads as one of a select's options, so a value
 * saved by an older host, the host's own fallback console or a hand edit
 * shows as that option. `match` is how the host compares text: exactly (the
 * default); ignoring letter case and surrounding spaces ('case'); that, and
 * reading - as _ ('dash'); or by its letters and digits alone, in any case
 * ('letters'). `spellings` maps other spellings, compared the same way, to
 * the option each means.
 */
export interface Aliases {
  match?: 'exact' | 'case' | 'dash' | 'letters';
  spellings?: Readonly<Record<string, string>>;
}

export interface Setting {
  key: string;
  label: string;
  /** One or two plain sentences: what it does and when to change it. */
  description: string;
  /** The finer points (limits, interactions, measurements), behind "More". */
  details?: string;
  category: CategoryId;
  /** A heading within the category, for grouping related settings. */
  group?: string;
  control: Control;
  /** For a select: other spellings the host reads as one of its options. */
  aliases?: Aliases;
  /** The value the Rust host uses when the key is not set. */
  default: ConfigValue;
  /** What the default does, in words, where its option label only says "Default". */
  defaultText?: string;
  /** Hide when not relevant, given the current (edited) values. */
  visibleWhen?: (values: Record<string, ConfigValue>) => boolean;
  /** Explain a hidden setting when search or review reveals it. */
  inactiveHint?: string;
  /**
   * visibleWhen asks what this host can run (its build, its GPU), not what
   * else is chosen: where it hides the setting, search and review show the
   * row without its control, so nobody sets what does nothing here. Gives
   * the row's tag, what the host lacks, from the same values.
   */
  hostGated?: (values: Record<string, ConfigValue>) => string;
  /**
   * Why this host cannot run it, when the host knows (with hostGated): shown
   * in place of inactiveHint, from the same values.
   */
  hostReason?: (values: Record<string, ConfigValue>) => string | undefined;
  /** Streams fail without a value while this holds, given the current (edited) values. */
  requiredWhen?: (values: Record<string, ConfigValue>) => boolean;
  /**
   * What the current value costs (measured delay, lost recovery) or risks
   * (ports open to the internet, lost progress), in one sentence, or
   * undefined when it costs nothing. Shown under the
   * control while the setting applies, and on the settings home when set by hand.
   */
  cautionWhen?: (values: Record<string, ConfigValue>) => string | undefined;
  /** Takes effect only after the host restarts. */
  restart?: boolean;
  advanced?: boolean;
  /** Defined for 3.0; hidden until its workstream ships it. */
  pending?: boolean;
  /** Edited inside the row of the setting with this key (its control's `daily`), so it has no row of its own. */
  partOf?: string;
}

/**
 * Not a setting: visibleWhen also gets the encoders Automatic would use on
 * this PC's GPUs ('amf', 'nvenc') under this name.
 */
export const NATIVE_ENCODERS = '@native_encoders';

/**
 * Not a setting: visibleWhen also gets, under this name, whether this
 * Windows can run Rubylight's virtual display driver (Windows 11 23H2 or
 * later): false where it cannot, absent where the host does not say.
 */
export const VIRTUAL_DISPLAY_OS = '@virtual_display_os';

/**
 * Not a setting: visibleWhen also gets the image enhancements this host
 * offers ('rtx_hdr', 'rtx_video') under this name. Only a host with the
 * NVIDIA components on an NVIDIA GPU whose driver supports them offers any.
 */
export const NVIDIA_ENHANCEMENTS = '@nvidia_enhancements';

/** Whether the host offers the image enhancement `name`. */
export function offersEnhancement(values: Record<string, ConfigValue>, name: string): boolean {
  const offered = values[NVIDIA_ENHANCEMENTS];
  return Array.isArray(offered) && offered.includes(name);
}

/**
 * Not a setting: visibleWhen also gets, under this name, what the RTX
 * settings wait for: 'no_gpu' on a PC without an NVIDIA graphics card,
 * otherwise the NVIDIA components' state ('missing', 'download', 'remove',
 * 'failed', 'ready').
 */
export const NVIDIA_COMPONENT = '@nvidia_component';

/**
 * The [NVIDIA_COMPONENT] value for the host's report of its NVIDIA components:
 * 'ready' while one is installed, whatever a download or check does meanwhile.
 */
export function componentGate(
  component: { state: string; installed?: boolean; nvidia_gpu: boolean } | null | undefined,
): string {
  if (!component) return 'missing';
  if (component.installed) return 'ready';
  return component.nvidia_gpu ? component.state : 'no_gpu';
}

/**
 * The tag of an RTX setting this host cannot run: what it needs, so a setting
 * found by search says what to do (issue #103).
 */
export function nvidiaGate(values: Record<string, ConfigValue>): string {
  const gate = values[NVIDIA_COMPONENT];
  if (gate === 'no_gpu') return 'Needs an NVIDIA graphics card';
  if (gate === 'ready') return 'Not available on this PC';
  return 'Needs the NVIDIA components';
}

/**
 * Not a setting: visibleWhen also gets, under this name, why the GPU and
 * driver do not offer each NVIDIA enhancement, by name ('rtx_hdr',
 * 'rtx_video'), as the host's check said.
 */
export const NVIDIA_REASONS = '@nvidia_reasons';

/**
 * The hostReason of an RTX setting: with the NVIDIA components installed and
 * the enhancement still not offered, what NVIDIA's check said (a crash, a
 * driver too old), so the row found by search says why (issues #103, #111).
 */
export function nvidiaReason(name: 'rtx_hdr' | 'rtx_video') {
  return (values: Record<string, ConfigValue>): string | undefined => {
    if (values[NVIDIA_COMPONENT] !== 'ready') return undefined;
    const reasons = values[NVIDIA_REASONS];
    const reason = reasons && typeof reasons === 'object' && !Array.isArray(reasons) ? reasons[name] : undefined;
    if (typeof reason !== 'string' || !reason.trim()) return undefined;
    const text = reason.trim();
    const sentence = text.charAt(0).toUpperCase() + text.slice(1);
    return /[.!?]$/.test(sentence) ? sentence : `${sentence}.`;
  };
}

export interface Schema {
  categories: Category[];
  settings: Setting[];
}
