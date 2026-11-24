// Core API types: auth, config, metadata, apps, clients, USB and game libraries.

export interface Ok {
  status: true;
}
/** One line of what restoring a settings backup changes. */
export interface BackupChange {
  item: 'settings' | 'apps' | 'covers' | 'game_settings' | 'devices' | 'pairing' | 'credentials' | 'webhooks';
  detail: string;
}
/** `POST /api/backup/inspect`: the checked upload's token and what restoring it changes. */
export interface BackupInspection {
  status: true;
  token: string;
  summary: {
    created_unix: number;
    host_version: string;
    /** The backup carries the pairing identity and credentials. */
    secrets: boolean;
    changes: BackupChange[];
  };
}
/** One Radeon setting as the driver reports it (the native API's model). */
export interface RadeonSetting {
  id: string;
  group: string;
  kind: 'toggle' | 'integer' | 'choice' | 'unknown';
  /** `configurable` is supported and writable. */
  state: 'configurable' | 'read_only' | 'needs_external_setup' | 'unsupported' | 'temporarily_unavailable' | 'unknown';
  value?: boolean | number | string;
  range?: { min: number; max: number; step: number };
  options?: { value: string; label: string }[];
  unit?: string;
  reason?: string;
  message?: string;
}
/** `GET /api/radeon`: what the Radeon controls may offer. It reads the driver and applies nothing. */
export interface RadeonPanel {
  status: true;
  state: RadeonSetting['state'];
  reason?: string | null;
  message?: string | null;
  gpus: { id: number; name: string; driver: string; settings: RadeonSetting[] }[];
}
export interface AuthStatus {
  status: true;
  authenticated: boolean;
  credentials_configured: boolean;
  login_required: boolean;
  /** On this PC while signed out: the program that sets a new sign-in with --creds. */
  creds_program?: string;
}
export interface IssuedSession {
  status: true;
  access_token: string;
  refresh_token: string;
  csrf_token: string;
  expires_in: number;
  refresh_expires_in: number;
  remember_me: boolean;
}
export interface WebSession {
  id: string;
  current: boolean;
  username: string;
  created_at: number;
  expires_at: number;
  refresh_expires_at: number;
  last_seen: number;
  remember_me: boolean;
  user_agent: string;
  remote_address: string;
  device_label: string;
}
export interface TokenScope {
  path: string;
  methods: string[];
}
export interface ApiToken {
  hash: string;
  username: string;
  created_at: number | string;
  scopes: TokenScope[];
}
export type ConfigValue = string | number | boolean | null | unknown[] | Record<string, unknown>;
export type Config = Record<string, ConfigValue>;

export interface CaptureDisplay {
  device_id: string;
  display_name: string;
  friendly_name: string;
  width: number;
  height: number;
  x: number;
  y: number;
  primary: boolean;
  adapter: string;
}
export interface AudioEndpoint {
  id: string;
  name: string;
  description: string;
  adapter: string;
  default: boolean;
  virtual_sink: boolean;
}
export interface Gpu {
  name: string;
  vendor: number;
  dedicated_memory: number;
  luid: [number, number];
  pnp_id: string | null;
}
/** `GET /api/nvidia-component`: the NVIDIA components RTX HDR and RTX Video enhancement need. */
export interface NvidiaComponent {
  /**
   * `missing` until downloaded; `download`, `check` (asking the GPU and driver
   * what they offer with it) or `remove` while one runs; `ready` when
   * installed; `failed` when the last change or check failed (`error`).
   */
  state: 'missing' | 'download' | 'check' | 'remove' | 'ready' | 'failed';
  /** Bytes downloaded of `total`. */
  done: number;
  total: number;
  error: string | null;
  /** Whether a component is installed and in use, whatever the job does. */
  installed: boolean;
  /** Whether the installed component is an earlier version's; the host fetches this version's. */
  outdated: boolean;
  /** Whether this PC has an NVIDIA graphics card. */
  nvidia_gpu: boolean;
}
export interface NvidiaEnhancements {
  /** The NVIDIA components, without which neither is offered. */
  component: NvidiaComponent;
  rtx_hdr: boolean;
  rtx_video: boolean;
  /** Why each enhancement this PC does not offer is missing, by name. */
  errors: Record<string, string>;
}

export interface Metadata {
  status: true;
  platform: 'windows' | 'linux';
  version: string;
  host_name: string;
  pc_address: string;
  pc_addresses: string[];
  paired_devices: number;
  enable_pairing: boolean;
  /** The console shows a pairing QR code (Settings, Pair with a QR code). */
  qr_pairing?: boolean;
  warnings: { code: string; message: string }[];
  encoder_status: {
    state: 'checking' | 'failed' | 'ready';
    h264: boolean;
    hevc: boolean;
    av1: boolean;
    pyrowave: boolean;
    /** The native YUV 4:4:4 modes the encoder check found, such as "HEVC 10-bit 4:4:4". */
    yuv444?: string[];
  };
  capture_status: {
    configured_backend: string;
    virtual_display_configured: boolean;
    displays: CaptureDisplay[] | null;
    error: string | null;
  };
  virtual_display: { capable: boolean; ready: boolean; reason: string; protocol: string };
  gpus: Gpu[] | null;
  gpu_error: string | null;
  audio_sinks: AudioEndpoint[] | null;
  audio_error: string | null;
  audio_enabled: boolean;
  /** `virtual_display_os`: whether this Windows can run the virtual display driver (Windows 11 23H2 or later). */
  features: { pyrowave: boolean; virtual_display: boolean; virtual_display_os: boolean; [key: string]: boolean };
  /** The NVIDIA image enhancements: what this PC offers and why the rest is missing. */
  nvidia_enhancements?: NvidiaEnhancements;
  credentials_exists: boolean;
  webrtc?: { enabled: boolean; running: boolean; discovery: boolean; port: number; media_port: number; error: string | null };
}

export interface PrepCommand {
  do: string;
  undo: string;
  elevated: boolean;
}
export interface App {
  name: string;
  cmd: string;
  'working-dir': string;
  'prep-cmd': PrepCommand[];
  uuid?: string;
  'image-path'?: string;
  detached?: string[];
  'config-overrides'?: Record<string, string | number | boolean>;
  /** competitive, smoothness, quality or custom. */
  'radeon-profile'?: string;
  /** The Radeon settings of a custom profile: setting id to value. */
  'radeon-settings'?: Record<string, string | number | boolean>;
  /** Keys in the game's .ini files the host sets for a stream and puts back. */
  'game-files'?: { path: string; section: string; key: string; value: string | number | boolean }[];
  /** The app's settings on one device, by device uuid: the keys above for that device. */
  'device-overrides'?: Record<string, Record<string, unknown>>;
  /** When Steam or Playnite last saw its game played, in seconds since the epoch; listed, never saved. */
  'last-played'?: number;
  [extra: string]: unknown;
}
export interface AppsDocument {
  status: true;
  apps: App[];
  env?: Record<string, string>;
  /** Synced games removed from the library (`source:id`), which no sync adds back. */
  'launcher-dismissed'?: string[];
}

export const PERM = {
  LIST_APPS: 0x01000000,
  VIEW_STREAMS: 0x02000000,
  LAUNCH_APPS: 0x04000000,
  CONTROLLER: 0x100,
  TOUCH: 0x200,
  PEN: 0x400,
  MOUSE: 0x800,
  KEYBOARD: 0x1000,
  CLIPBOARD_WRITE: 0x10000,
  CLIPBOARD_READ: 0x20000,
  FILE_UPLOAD: 0x40000,
  FILE_DOWNLOAD: 0x80000,
  SERVER_CMD: 0x100000,
} as const;
export const PERM_ALL = 0x071f1f00;
/** Full control: everything but file transfer, which a device is granted on its own. */
export const FULL_CONTROL = PERM_ALL & ~(PERM.FILE_UPLOAD | PERM.FILE_DOWNLOAD);

// Per-device settings: null clears one, so the host setting applies.
export interface Client {
  uuid: string;
  name: string;
  perm: number;
  enabled: boolean;
  connected: boolean;
  /** Set for a device paired over WebRTC; the secret never leaves the host. */
  webrtc?: { clientId: string };
  network_loss_routes?: { address: string; expires_in_seconds: number; sessions_until_probe: number }[];
  /** The User-Agent of the device's latest /serverinfo since the host started. Read-only. */
  client_agent?: string | null;
  /** A known problem with this client build. The host never refuses the device for it. */
  client_warning?: { problem: string; link: string } | null;
  output_name_override?: string | null;
  display_mode?: string | null;
  always_use_virtual_display?: boolean | string | null;
  remember_display_mode?: boolean | string | null;
  remembered_resolution?: string | null;
  remembered_scale?: number | string | null;
  background_stream?: boolean | string | null;
  /** Can join co-op games other devices started. Unset (or null) follows Launch apps; set, only true and "true" join. */
  coop_join?: boolean | string | null;
  virtual_display_mode?: string | null;
  virtual_display_layout?: string | null;
  virtual_display_layout_set_by_user?: boolean | null;
  /** The app or host layout a layout learned from a stream replaced, so the console can say it is learned. */
  virtual_display_layout_learned_over?: string | null;
  prefer_10bit_sdr?: boolean | string | null;
  hdr_profile?: string | null;
  allow_client_commands?: boolean | string | null;
  /** Host control (Radeon settings, close game). Sent as true or null; the host refuses anything else. */
  host_control?: boolean | string | null;
  /** Approve new devices from this device. Sent as true or null; only a JSON true grants it. */
  pairing_approve?: boolean | null;
  /** Sleep PC from this device. Sent as true or null; the host refuses anything else. */
  allow_sleep?: boolean | string | null;
  /** Restart and shut down PC from this device. Sent as true or null; only a JSON true grants it. */
  allow_restart_shutdown?: boolean | null;
  /** USB forwarding from this device. Sent as true or null; only a JSON true grants it. */
  allow_usb?: boolean | null;
  /** Shared folder grants by folder id. Null clears them. */
  shared_folders?: Record<string, 'read' | 'write'> | null;
  do?: unknown;
  undo?: unknown;
  [extra: string]: unknown;
}
/** A USB device a paired device forwards to this PC (`GET /api/usb`). */
export interface UsbAttachment {
  id: number;
  /** The paired device's name. */
  client: string;
  name: string;
  vid: number;
  pid: number;
  class: number;
  speed: number;
  busid: string;
  /** `detach_failed`: the driver kept it; detaching it again retries. */
  state: 'waiting' | 'opening' | 'attached' | 'detaching' | 'detach_failed';
  /** The driver's hub port once attached. */
  port: number | null;
  since_unix_ms: number;
  error: string | null;
}
export interface PendingPairing {
  uniqueid: string;
  name: string;
  /** Where the request came from: the name is the device's to choose, the address is not. */
  address?: string;
  /** The request comes from outside the local network. */
  internet?: boolean;
  age_seconds: number;
  /** The request comes from a WebRTC TV. */
  webrtc?: boolean;
}

export type LibrarySource = 'steam' | 'epic' | 'battlenet' | 'ea' | 'ubisoft' | 'gog' | 'microsoft' | 'local';
export interface DiscoveredGame {
  source: LibrarySource;
  id: string;
  uuid: string;
  name: string;
  install_dir: string;
  already_added: boolean;
  unavailable_reason: string | null;
  has_cover: boolean;
}
export interface LibraryDiscovery {
  status: true;
  sources: { source: LibrarySource; name: string; state: 'available' | 'unavailable' | 'error' | 'disabled'; errors: string[] }[];
  games: DiscoveredGame[];
}
export interface LibraryImportResult {
  source: LibrarySource;
  id: string;
  result: 'added' | 'already_present' | 'failed';
  error: string | null;
}

export interface SteamStatus {
  status: true;
  enabled: boolean;
  /** Whether Steam is installed on the host. */
  available: boolean;
  game_count: number;
  importable_game_count: number;
  selected_game_count: number;
  auto_sync: boolean;
  sync_all_installed: boolean;
  recent_games: number;
}

export interface PlayniteStatus {
  status: true;
  enabled: boolean;
  /** Playnite is running. */
  active: boolean;
  /** Playnite was found on the host. */
  available: boolean;
  /** The plugin is in Playnite's extensions. */
  installed: boolean;
  installed_version: string | null;
  packaged_version: string | null;
  update_available: boolean;
  auto_sync: boolean;
  game_count: number;
  synced_seconds_ago: number | null;
}
