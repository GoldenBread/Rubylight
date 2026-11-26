// Settings › Network › Away wake relay: the relay list, its staleness, the off and empty states,
// and the console routes it calls.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const Panel = (await server.ssrLoadModule('/src/components/network/WakeRelayPanel.svelte')).default;
const { STALE_SECONDS, RELAY_COMMAND, relayName, relaySeen, relayStale } = await server.ssrLoadModule(
  '/src/components/network/wake-relay.ts',
);
const { settings } = await server.ssrLoadModule('/src/lib/schema/basics.ts');
const { api, ApiError, setCsrfToken } = await server.ssrLoadModule('/src/lib/api.ts');

const html = (initial) => render(Panel, { props: { initial } }).body;
const now = () => Math.floor(Date.now() / 1000);
const relay = (name, lastSeenAgo, enrolledAgo = lastSeenAgo) => ({
  id: name.padEnd(64, '0').slice(0, 64),
  name,
  enrolled_unix: now() - enrolledAgo,
  last_seen_unix: lastSeenAgo === null ? 0 : now() - lastSeenAgo,
});

test('a relay counts as not refreshing once it missed three five-minute refreshes', () => {
  const at = 1_800_000_000;
  const seen = (ago) => ({ id: 'a', name: 'Mini PC', enrolled_unix: at - 86_400, last_seen_unix: at - ago });
  assert.equal(STALE_SECONDS, 15 * 60);
  assert.equal(relayStale(seen(4 * 60), at), false);
  assert.equal(relayStale(seen(STALE_SECONDS), at), false);
  assert.equal(relayStale(seen(STALE_SECONDS + 1), at), true);
  // Never seen since enrolling: counted from the enrollment.
  assert.equal(relayStale({ id: 'b', name: '', enrolled_unix: at - 60, last_seen_unix: 0 }, at), false);
  assert.equal(relayStale({ id: 'b', name: '', enrolled_unix: at - 3600, last_seen_unix: 0 }, at), true);
});

test('a relay is named, and says when it last checked in', () => {
  assert.equal(relayName({ name: '  ' }), 'Unnamed device');
  assert.equal(relayName({ name: 'Home server' }), 'Home server');
  assert.equal(relaySeen(relay('Mini PC', 10)), 'Last checked in just now');
  assert.equal(relaySeen(relay('Mini PC', 20 * 60)), 'Last checked in 20 min ago');
  assert.equal(relaySeen({ name: 'x', enrolled_unix: 0, last_seen_unix: 0 }), 'Not checked in yet');
});

test('off, the panel says what to turn on and lists no setup steps', () => {
  const out = html({ enabled: false, epoch: 1, relays: [] });
  assert.match(out, /Away wake relay/);
  assert.match(out, /Turn on Play over the internet, then Away wake relay/);
  assert.doesNotMatch(out, /No wake relay yet|Rotate keys|Remove/);
  assert.doesNotMatch(out, new RegExp(RELAY_COMMAND));
});

test('on without a relay, the empty state shows how to add one', () => {
  const out = html({ enabled: true, epoch: 1, relays: [] });
  assert.match(out, /No wake relay yet/);
  assert.match(out, /pair it with this PC/);
  assert.match(out, /Host control/);
  assert.match(out, /href="\/devices"/);
  assert.match(out, /nightjar wake-relay/);
  assert.doesNotMatch(out, /Rotate keys|Turn on Play over the internet/);
});

test('enrolled relays show their state, Remove and Rotate keys', () => {
  const out = html({ enabled: true, epoch: 2, relays: [relay('Mini PC', 60), relay('Home server', 40 * 60, 86_400)] });
  assert.match(out, /Mini PC/);
  assert.match(out, /Home server/);
  assert.match(out, /data-status="ready"[^]*Ready/);
  assert.match(out, /data-status="warning"[^]*Not refreshing/);
  assert.match(out, /Last checked in 40 min ago/);
  assert.match(out, /still running/);
  assert.equal(out.match(/>\s*Remove\s*</g)?.length, 2);
  assert.match(out, /Rotate keys/);
  assert.match(out, /Add another relay/);
  assert.doesNotMatch(out, /No wake relay yet/);
});

test('relays kept while the feature is off can still be removed', () => {
  const out = html({ enabled: false, epoch: 1, relays: [relay('Mini PC', 30 * 60)] });
  assert.match(out, /Turn on Play over the internet/);
  assert.match(out, /Mini PC/);
  assert.match(out, />\s*Remove\s*</);
  assert.doesNotMatch(out, /Add another relay|still running/);
});

test('Remove and Rotate keys post to the console routes, and a refusal keeps the host\'s words', async () => {
  const seen = [];
  setCsrfToken('token');
  const answer = '{"enabled":true,"epoch":3,"relays":[]}';
  globalThis.fetch = async (path, init) => {
    seen.push({ path, method: init.method ?? 'GET', body: init.body, csrf: init.headers?.['X-CSRF-Token'] });
    if (path === '/api/wake-relay/revoke' && JSON.parse(init.body).id === 'gone') {
      return new Response('{"error":"That wake relay is not enrolled on this PC."}', { status: 400 });
    }
    return new Response(answer, { status: 200 });
  };
  try {
    assert.deepEqual(await api.wakeRelay.status(), JSON.parse(answer));
    assert.deepEqual(await api.wakeRelay.revoke('abc'), JSON.parse(answer));
    assert.deepEqual(await api.wakeRelay.rotate(), JSON.parse(answer));
    await assert.rejects(api.wakeRelay.revoke('gone'), (error) => {
      assert.ok(error instanceof ApiError);
      assert.equal(error.status, 400);
      assert.equal(error.message, 'That wake relay is not enrolled on this PC.');
      return true;
    });
    assert.deepEqual(
      seen.slice(0, 3).map(({ path, method, body, csrf }) => [path, method, body, csrf]),
      [
        ['/api/wake-relay', 'GET', undefined, undefined],
        ['/api/wake-relay/revoke', 'POST', '{"id":"abc"}', 'token'],
        ['/api/wake-relay/rotate', 'POST', '{}', 'token'],
      ],
    );
  } finally {
    delete globalThis.fetch;
  }
});

test('Away wake relay is a Network setting beside Play over the internet, and Settings shows the panel there', () => {
  const setting = settings.find((entry) => entry.key === 'away_wake_relay');
  assert.equal(setting.category, 'network');
  assert.equal(setting.default, false);
  assert.equal(setting.visibleWhen({ internet_play: false }), false);
  assert.equal(setting.visibleWhen({ internet_play: true }), true);
  const page = readFileSync(new URL('./src/pages/Settings.svelte', import.meta.url), 'utf8');
  assert.match(page, /<InternetPlayPanel \/>\s*<WakeRelayPanel saved=\{stored\} \/>/);
  const panel = readFileSync(new URL('./src/components/network/WakeRelayPanel.svelte', import.meta.url), 'utf8');
  assert.doesNotMatch(panel, /experimental|beta\b|preview|nucleus|\bANR\b/i);
});
