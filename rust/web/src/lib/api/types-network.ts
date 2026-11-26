// Guests, internet play, health, updates, logs, power, pairing and webhook API types.

/** What a guest of the Stream page may do besides watching and listening. */
export type GuestPermission = 'view' | 'controller' | 'controller-keyboard-mouse';

/** An invite link, by its public id; the host never has the token again. Times in Unix seconds. */
export interface GuestInvite {
  id: string;
  permission: GuestPermission;
  created: number;
  expires: number;
  /** Made for guests on the internet: the guest ports stay mapped on the router while it works. */
  internet?: boolean;
}

/** One guest port and what the router did with it. */
export interface GuestPort {
  protocol: string;
  port: number;
  /** The router's port for it; null when not mapped. */
  external: number | null;
  reason: string | null;
}

/** `GET`/`POST /api/guests/internet`: what a link for guests on the internet can use. */
export interface GuestReach {
  /** The router has not been asked yet since the dialog asked for the ports. */
  checking: boolean;
  /** Whether the host maps ports at all (not when it listens on this PC only). */
  runs: boolean;
  /** UPnP, PCP or NAT-PMP. */
  method: string | null;
  /** Why nothing could be mapped. */
  error: string | null;
  ports: GuestPort[];
  publicIpv4: string | null;
  routerIpv4: string | null;
  ipv6: string | null;
  /** The address and port the link carries; null when nothing on the internet reaches the guest ports. */
  address: string | null;
  notes: string[];
  /** Play over the internet runs: paired Nightjar devices reach the PC without forwarded ports. */
  internetPlay: boolean;
  /** The host listens on IPv6, so the router can open the guest ports to IPv6 guests. */
  ipv6Listening: boolean;
  /** When another device joins the running game: off keeps a paired friend from joining as a guest. */
  guestJoin: 'off' | 'ask' | 'allow';
}

export interface GuestEntry {
  id: number;
  name: string;
  permission: GuestPermission;
  /** The invite it joined with. */
  invite: string;
  since: number;
}

/** `GET /api/session-guests`: paired devices that joined the running game as guests (docs/multiclient.md). */
export type GuestJoinPolicy = 'ask' | 'allow' | 'off';
export interface DeviceGuest {
  id: number;
  name: string;
  permission: GuestPermission;
  /** Whether its stream runs now. */
  streaming: boolean;
  /** Unix seconds it was let in. */
  since: number;
  /** Unix seconds its time ends; null lasts until the game ends. */
  expires: number | null;
}
export interface JoinWaiting {
  id: number;
  name: string;
  /** Until the join gives up. */
  seconds_left: number;
}
/** `GET /api/session-guests/invites`: invites to play over the internet (docs/multiclient.md). */
export interface InviteLink {
  id: number;
  permission: GuestPermission;
  /** Unix seconds. */
  created: number;
  /** Unix seconds the link, and the access it gives, end. */
  expires: number;
}
export interface InvitedFriend {
  uuid: string;
  name: string;
  permission: GuestPermission;
  /** Unix seconds its access ends and the host removes it. */
  until: number;
}
export interface GuestInvites {
  links: InviteLink[];
  friends: InvitedFriend[];
  max_links: number;
  default_seconds: number;
}
/** A new invite: the link is shown once and never kept by the host. */
export interface InviteCreated {
  id: number;
  link: string;
  expires: number;
  permission: GuestPermission;
}
export interface SessionGuestsStatus {
  policy: GuestJoinPolicy;
  default_permission: GuestPermission;
  /** The device playing, when a game runs. */
  owner: string | null;
  owner_name: string | null;
  max_guests: number;
  version: number;
  /** The guest holding the keyboard and mouse; null while the device playing holds them. */
  keyboard_mouse: number | null;
  guests: DeviceGuest[];
  waiting: JoinWaiting[];
}

/** `GET /api/guests`: the Stream page's invite links and the guests watching. */
export interface GuestsStatus {
  /** Whether the console's stream runs, which guests join. */
  streaming: boolean;
  max_guests: number;
  /** Links still working, and expired ones a guest still watches with. */
  invites: GuestInvite[];
  guests: GuestEntry[];
  /** The guest page's own TCP port; null when the host's base port is unreadable. */
  port: number | null;
  /** Each guest seat's UDP media port. */
  media_ports?: number[];
}

/** `POST /api/guests/invite`: the new link's token, shown once. */
export interface CreatedInvite {
  invite: GuestInvite;
  token: string;
}

/** One streaming port as the router mapped (or refused) it. */
export interface InternetPort {
  protocol: 'TCP' | 'UDP';
  port: number;
  external: number | null;
  reason: string | null;
}
/** `GET /api/internet-play`: port mapping, STUN and the rendezvous for Play over the internet. */
export interface InternetPlayStatus {
  enabled: boolean;
  portMapping: boolean;
  mapping: {
    enabled: boolean;
    method: string | null;
    externalIpv4: string | null;
    mapped: InternetPort[];
    failed: InternetPort[];
    error: string | null;
    ipv6Method: string | null;
    ipv6Pinholes: number;
    ipv6Error: string | null;
    /** Browser guests' ports, mapped only while a link made for the internet needs them. */
    guestMapped?: InternetPort[];
    guestFailed?: InternetPort[];
  };
  externalIp: string | null;
  rendezvous: {
    public: string[];
    nat: 'unknown' | 'none' | 'cone' | 'symmetric';
    stunError: string | null;
    devices: number;
    published: number;
    dhtError: string | null;
    paths: { device: string; peer: string | null; connected: boolean }[];
    /** Devices whose path did not start lately, and why. */
    pathErrors: { device: string; error: string }[];
  } | null;
  notes: string[];
}
export interface VulkanLayerStatus {
  status: true;
  installed: boolean;
  enabled: boolean;
  available: boolean;
  active: boolean;
}
export type CrashStatus =
  | { available: false; dismissed: false }
  | {
      available: true;
      filename: string;
      process: string;
      size_bytes: number;
      captured_at: string;
      age_seconds: number;
      dismissed: boolean;
    };
export interface Release {
  tag_name: string;
  name: string;
  html_url: string;
  prerelease: boolean;
  published_at: string;
  body: string;
}
export interface UpdatesState {
  status: true;
  checking: boolean;
  check_failed: boolean;
  check_error?: string | null;
  checked_at: number;
  /** The last successful check before the host started, while it has not checked since (checked_at is 0). */
  previous_check_at?: number | null;
  /** Unix seconds of the next automatic check; null when automatic checks are off. */
  next_check_at?: number | null;
  /** The check is due but held back by an active or pending stream. */
  deferred?: boolean;
  releases: Release[];
  update_available?: boolean;
  latest_version?: string | null;
  install_supported: boolean;
  auto_update: boolean;
  update_check_interval: number;
  /** HH:MM when the automatic check runs once a day at that time; null when the interval decides. */
  update_check_time?: string | null;
  phase?: 'idle' | 'waiting' | 'downloading' | 'ready' | 'installing' | 'failed';
  queued_version?: string | null;
  /** Install now: does not wait for an idle host and ends a running stream. */
  now?: boolean;
  /** A device's Install now waits for another device's stream, which it never ends: who streams. */
  waiting_for?: string | null;
  downloaded_bytes?: number;
  download_size?: number;
  error?: string | null;
  last_install?: { version: string; phase: string; error?: string } | null;
}
export interface BrowseEntry {
  name: string;
  path: string;
  type: 'directory' | 'file';
}
export interface BrowseListing {
  /** The folder listed; empty for the list of drives. */
  path: string;
  /** Empty at a drive's root, which goes back to the drives. */
  parent: string;
  entries: BrowseEntry[];
}
/** Which files a listing includes; folders are always listed. */
export type BrowseType = 'executable' | 'file' | 'directory';
export interface LogFinding {
  id: string;
  category: 'encoder' | 'capture' | 'network' | 'driver';
  level: 'warn' | 'error';
  title: string;
  explanation: string;
  fix: string;
  /** The troubleshooting guide's section for it. */
  link: string;
  /** The setting the fix names, by its key; the finding links to it. */
  setting?: string | null;
  count: number;
  first_seen: string;
  last_seen: string;
  /** The latest matching line, without its time and level. */
  sample: string;
}
export interface LogAnalysis {
  status: true;
  /** Newest first. */
  findings: LogFinding[];
  /** The first and last time in the part of the log that was read. */
  since: string | null;
  until: string | null;
  read_bytes: number;
}
export interface LogChunk {
  status: true;
  /** Byte offset to ask for next. */
  offset: number;
  size: number;
  /** The log was rotated or truncated since `offset`; `text` starts afresh. */
  reset: boolean;
  /** Optional fingerprint of the log head to detect rotation even past `offset`. */
  head?: string;
  text: string;
}
/** One wake setting of an adapter; ids and states from a newer host are shown by their state. */
export interface WakeCheck {
  id: string;
  state: 'ok' | 'warning' | 'error' | 'unknown';
}
/** A physical adapter a magic packet can wake the PC through. */
export interface WakeTarget {
  mac: string;
  kind: 'ethernet' | 'wifi' | 'unknown';
  connected: boolean;
  /** This console's request came in through this adapter. */
  route: boolean;
  broadcasts: string[];
  state: 'ready' | 'needs_setup' | 'experimental' | 'unknown';
  checks: WakeCheck[];
}
export type IdleSleepState = 'off' | 'armed' | 'blocked' | 'unsupported' | 'unknown';
export interface PowerStatus {
  status: true;
  wake: {
    revision: number;
    /** The MAC `/serverinfo` reports; null when no physical adapter is connected. */
    primary: string | null;
    targets: WakeTarget[];
    system: {
      sleep: 's3' | 'none' | 'unknown';
      modern_standby: boolean;
      hibernate: boolean;
      fast_startup: 'active' | 'inactive' | 'off' | 'unknown';
      shutdown_wake: 'firmware_unknown' | 'unknown';
    };
  };
  /**
   * Why `wake.primary` is null: opened on this PC without a default route
   * (`no_route`) or with an unreadable one (`route_error`, with `error`), or the
   * address leads to no connected physical adapter (`unmapped`). Absent from older hosts.
   */
  primary_missing?: { reason: 'no_route' | 'route_error' | 'unmapped'; error: string | null } | null;
  idle_sleep: {
    state: IdleSleepState;
    /** 0 when off. */
    minutes: number;
    /** Why it is blocked or unsupported, or, while armed, what it waits for. */
    reason: string | null;
    /** While armed: seconds until the PC sleeps if nothing changes. */
    remaining_seconds: number | null;
  };
  /** Unix seconds; `error` when Windows refused to sleep. */
  last_sleep: { at: number; error: string | null } | null;
  fix: {
    running: boolean;
    /** The changes a fix would make; null when the adapters could not be read (`error`). */
    changes: string[] | null;
    /** Adapters that need a change the host can't make, and why. */
    skipped: string[] | null;
    error: string | null;
    /** Fast Startup is active: turning it off is its own choice. */
    fast_startup_off_offered: boolean;
    last: {
      at: number;
      changes?: string[];
      restarted?: string[];
      after_reboot?: string[];
      errors: string[];
    } | null;
  };
}
export interface PowerFixResult {
  status: true;
  /** Empty when there was nothing to change (200); applied about a second later otherwise (202). */
  changes: string[];
  skipped: string[];
}

/** What a device may do once paired (`butterpollo_core::state::pairing_access`). The first device gets everything. */
export type PairingAccess = 'play' | 'view' | 'all';
/** A pairing QR code for one of this PC's addresses (`rubylight://pair?...`). */
export interface PairingCode {
  address: string;
  /** The code as an SVG document, rendered by the host. */
  svg: string;
  uri: string;
}
export interface WebhookStatus {
  event: string;
  delivery: string;
  time_unix_ms: number;
  ok: boolean;
  http_status: number | null;
  error: string | null;
  attempts: number;
}
export interface WebhookEntry {
  url: string;
  events: string[];
  /** The last delivery since the host started, or null. */
  last: WebhookStatus | null;
}
export interface WebhookList {
  status: true;
  events: string[];
  max: number;
  hooks: WebhookEntry[];
  /** Why the saved list could not be read, when it could not. */
  error: string | null;
}
export interface WebhookSave {
  url: string;
  events: string[];
  /** The URL of the hook being edited; absent to add one. */
  previous?: string;
  rotate_secret?: boolean;
}
