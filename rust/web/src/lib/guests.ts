// The Stream page's guests (docs/browser-guests.md): what an invite may do, how long it lasts and
// the link it makes. Pure, so it is tested in Node (Guests.test.mjs).
import type { GuestPermission, GuestPort, GuestReach } from './api';

export const PERMISSIONS: readonly { value: GuestPermission; label: string }[] = [
  { value: 'view', label: 'View only' },
  { value: 'controller', label: 'Controller' },
  { value: 'controller-keyboard-mouse', label: 'Controller, keyboard and mouse' },
];

export const permissionLabel = (permission: GuestPermission) => PERMISSIONS.find((p) => p.value === permission)?.label ?? permission;

/** The host keeps a link five minutes to seven days (`butterpollo_core::webrtc::guests`). */
export const MIN_SECONDS = 5 * 60;
export const MAX_SECONDS = 7 * 86_400;

export type Expiry = '1h' | '24h' | 'custom';
export const EXPIRIES: readonly { value: Expiry; label: string }[] = [
  { value: '1h', label: '1 hour' },
  { value: '24h', label: '24 hours' },
  { value: 'custom', label: 'Custom' },
];
export const UNITS: readonly { value: string; label: string }[] = [
  { value: '60', label: 'Minutes' },
  { value: '3600', label: 'Hours' },
  { value: '86400', label: 'Days' },
];

/** How long a link lasts, in seconds; null when the custom amount is outside what the host keeps. */
export function lifetime(expiry: Expiry, amount: number, unit: number): number | null {
  if (expiry === '1h') return 3600;
  if (expiry === '24h') return 86_400;
  const seconds = Math.round(amount * unit);
  return Number.isFinite(seconds) && seconds >= MIN_SECONDS && seconds <= MAX_SECONDS ? seconds : null;
}

const loopback = (hostname: string) => {
  const name = hostname.replace(/^\[|\]$/g, '').toLowerCase();
  return name === 'localhost' || name === '::1' || name.endsWith('.localhost') || /^127\./.test(name);
};

/**
 * The address a guest opens the link at: the console's host, or for a console opened on this PC
 * (`localhost`), the PC's first LAN address; on the guest page's own port.
 */
export function linkAddress(page: { hostname: string }, addresses: readonly string[], port: number): string {
  const host = loopback(page.hostname) && addresses[0] ? addresses[0] : page.hostname;
  return `${host}:${port}`;
}

/** `address` with the guest port when it names none: a host name alone would open port 443. */
export function withPort(address: string, port: number): string {
  const trimmed = address.trim();
  return /:\d{1,5}$/.test(trimmed) && !/^[^[]*:[^\]]*:/.test(trimmed) ? trimmed : `${trimmed}:${port}`;
}

/** An address the link can carry: a host name or IP with an optional port, nothing else. */
export function validAddress(address: string): boolean {
  return /^(\[[0-9a-f:.]+\]|[a-z0-9.-]+)(:\d{1,5})?$/i.test(address.trim());
}

/** Why a guest couldn't open a link at `address`, or undefined when one can. */
export function addressProblem(address: string): string | undefined {
  if (!validAddress(address)) return 'A host name or IP address, with an optional port';
  if (loopback(address.trim().replace(/:\d{1,5}$/, ''))) return 'Only this PC opens this address';
  return undefined;
}

/** Where the guests are: the PC's own network (or a VPN), or the internet. */
export type Audience = 'local' | 'internet';
export const AUDIENCES: readonly { value: Audience; label: string }[] = [
  { value: 'local', label: 'This network or a VPN' },
  { value: 'internet', label: 'On the internet' },
];

/** "TCP 47991, UDP 8002-8005" for the guest ports, as the dialog names them. */
export function guestPorts(port: number | null, mediaPorts: readonly number[]): string {
  const sorted = [...mediaPorts].sort((a, b) => a - b);
  const contiguous = sorted.every((p, i) => i === 0 || p === (sorted[i - 1] ?? NaN) + 1);
  const udp = !sorted.length ? '' : contiguous && sorted.length > 1 ? `UDP ${sorted[0]}-${sorted[sorted.length - 1]}` : sorted.map((p) => `UDP ${p}`).join(', ');
  return [port === null ? '' : `TCP ${port}`, udp].filter(Boolean).join(', ');
}

/** One line per guest port: mapped, or not and why. */
export function portLines(ports: readonly GuestPort[]): { label: string; ok: boolean; detail: string }[] {
  return ports.map((p) => ({
    label: `${p.protocol} ${p.port}`,
    ok: p.external !== null,
    detail:
      p.external === null
        ? (p.reason ?? 'not mapped')
        : p.external === p.port
          ? 'forwarded'
          : `forwarded as ${p.external}, which guests can't use`,
  }));
}

/** What the dialog says about reaching this PC from the internet, in one line. */
export function reachVerdict(reach: GuestReach | null): { tone: 'neutral' | 'ok' | 'warn'; text: string } {
  if (!reach) return { tone: 'neutral', text: 'Asking the router to forward the guest ports…' };
  if (!reach.runs) return { tone: 'warn', text: 'Rubylight listens on this PC only, so nothing on the internet can reach it.' };
  if (reach.address && !reach.checking) {
    const ipv6 = reach.address.startsWith('[');
    return {
      tone: 'ok',
      text: ipv6
        ? `Guests on the internet reach this PC over IPv6 at ${reach.address}. They need IPv6 too.`
        : `Guests on the internet reach this PC at ${reach.address}.`,
    };
  }
  if (reach.checking) return { tone: 'neutral', text: 'Asking the router to forward the guest ports…' };
  return { tone: 'warn', text: 'Nothing on the internet can reach this PC\'s guest ports.' };
}

/** What works instead when the router forwards nothing, plainly. */
export function reachAlternatives(reach: GuestReach, ports: string): string[] {
  const lines = [
    'Invite them over Tailscale or another VPN: pick "This network or a VPN" and use this PC\'s VPN address.',
    reach.ipv6Listening
      ? `Over IPv6, if you both have it: allow ${ports} in to this PC in your router's IPv6 firewall, then type the PC's IPv6 address in brackets below.`
      : 'Over IPv6, if you both have it: set IP versions to IPv4 and IPv6 (All settings › Advanced › Expert settings › Network › Ports and addresses) and restart Rubylight, so the router can open the guest ports to IPv6 guests.',
    `Or forward ${ports} on your router by hand, then type your public address or a DNS name for it below.`,
  ];
  if (reach.internetPlay) {
    // A paired friend joins as a guest only when When another device joins the running game
    // lets a guest in; name the two values then, and keep the line short when it already does.
    const setting =
      reach.guestJoin === 'off'
        ? ' with When another device joins the running game set to Ask the player or Let in without asking'
        : '';
    lines.push(`A friend with Nightjar paired to this PC can join your game through Play over the internet instead${setting}.`);
  }
  return lines;
}

/** The address the link starts with: the one the host found for the audience, else the page's. */
export function suggestedAddress(audience: Audience, local: string, reach: GuestReach | null): string {
  if (audience === 'local') return local;
  return reach?.address ?? '';
}

/** The token goes in the fragment: browsers never send it, so no log or proxy sees it. */
export function guestLink(address: string, token: string): string {
  return `https://${address.trim()}/guest/#${token}`;
}

/** "Expires in 59 min", "in 23 h", "in 6 days" from Unix seconds; "Expired" once it has. */
export function expiresIn(expires: number, now = Date.now() / 1000): string {
  if (expires <= now) return 'expired';
  const left = expires - now;
  if (left < 3600) return `in ${Math.max(1, Math.ceil(left / 60))} min`;
  if (left < 2 * 86_400) return `in ${Math.round(left / 3600)} h`;
  return `in ${Math.round(left / 86_400)} days`;
}
