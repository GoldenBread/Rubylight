// Who is signed in, and what the host is: shared by the shell and pages.
import { ApiError, api, type Metadata, type PendingPairing } from './api';
import { navigate, route } from './router.svelte';

export const session = $state({
  state: 'checking' as 'checking' | 'unavailable' | 'signed-out' | 'needs-setup' | 'signed-in',
  error: '',
  metadata: null as Metadata | null,
  pending: [] as PendingPairing[],
  /**
   * Signed in, but the host stopped answering (it stopped or restarts): the shell shows
   * "Cannot connect" over the page, which keeps its unsaved edits, until the host answers.
   */
  reconnecting: false,
});

const PUBLIC = ['/login', '/setup'];

/** A request that never got an answer: the host is not running or not reachable. A refusal is an answer. */
export function unreachable(error: unknown): boolean {
  return error instanceof TypeError || (error instanceof ApiError && error.status === 0);
}

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

export async function checkSession() {
  try {
    const status = await api.auth.status();
    session.error = '';
    session.reconnecting = false;
    if (!status.credentials_configured) {
      session.state = 'needs-setup';
      if (route.path !== '/setup') navigate('/setup', { replace: true });
      return;
    }
    if (!status.authenticated) {
      signedOut();
      return;
    }
    await api.auth.resume();
    session.state = 'signed-in';
    if (PUBLIC.includes(route.path)) navigate('/', { replace: true });
    refreshMetadata().catch(() => {});
    // A device waiting for a PIN shows at once, not on the shell's next poll.
    refreshPending().catch(() => {});
  } catch (error) {
    session.error = message(error);
    // A signed-in console keeps its page under the notice; it is back when the host answers.
    if (session.state === 'signed-in' && unreachable(error)) session.reconnecting = true;
    else session.state = 'unavailable';
  }
}

export function signedOut() {
  session.state = 'signed-out';
  session.metadata = null;
  session.pending = [];
  if (route.path !== '/login') {
    const back = route.path === '/' ? '' : `?next=${encodeURIComponent(route.path + route.search)}`;
    navigate(`/login${back}`, { replace: true, force: true });
  }
}

export async function refreshMetadata() {
  session.metadata = await api.metadata();
}

/** Pairing requests waiting for a PIN, shown on every page. */
export async function refreshPending() {
  if (session.state !== 'signed-in') return;
  session.pending = (await api.clients.pending()).requests;
}

/**
 * The shell's poll: the pairing requests while the host answers. A request that gets no
 * answer at all asks the host for the sign-in state, which shows "Cannot connect" if that
 * fails too; from then on the poll asks until the host is back.
 */
export async function heartbeat(): Promise<void> {
  if (session.state === 'unavailable' || session.reconnecting) return checkSession();
  try {
    await refreshPending();
  } catch (error) {
    if (unreachable(error)) await checkSession();
  }
}
