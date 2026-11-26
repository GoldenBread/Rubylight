// The away wake relay panel's words: a relay's name, when it last checked in and whether it
// still fetches its keys. A relay refreshes every five minutes while the PC is awake.
import type { WakeRelayPeer } from '../../lib/api';
import { ago } from '../../lib/format';

/** Three missed refreshes: past this a relay counts as not refreshing. */
export const STALE_SECONDS = 15 * 60;

/** The command an always-on computer runs to act as a relay. */
export const RELAY_COMMAND = 'nightjar wake-relay';

export function relayName(relay: WakeRelayPeer): string {
  return relay.name.trim() || 'Unnamed device';
}

/** When the relay last fetched its keys, or enrolled when it never has (Unix seconds). */
function lastContact(relay: WakeRelayPeer): number {
  return relay.last_seen_unix || relay.enrolled_unix;
}

/** Whether the relay has not fetched its keys for more than `STALE_SECONDS` at `now` (Unix seconds). */
export function relayStale(relay: WakeRelayPeer, now: number): boolean {
  return now - lastContact(relay) > STALE_SECONDS;
}

/** "Last checked in 3 min ago", relative to the browser's clock. */
export function relaySeen(relay: WakeRelayPeer): string {
  const at = lastContact(relay);
  return at ? `Last checked in ${ago(at)}` : 'Not checked in yet';
}
