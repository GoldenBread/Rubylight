import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const { default: Sessions } = await server.ssrLoadModule('/src/pages/Sessions.svelte');
const {
  aStreamEnded,
  bitrateText,
  categoryLabel,
  codecName,
  durationText,
  fpsText,
  lossText,
  matches,
  p95Text,
  pictureText,
  reasonLabel,
  reasonTone,
} = await server.ssrLoadModule('/src/lib/session-history.ts');

const source = (path) => readFileSync(new URL(`./src/${path}`, import.meta.url), 'utf8');

const entry = (changes = {}) => ({
  id: 'stream-1',
  device: 'phone',
  device_name: 'Pixel 9',
  app_id: 3,
  app: 'Portal 2',
  role: 'stream',
  transport: 'moonlight',
  codec: 'hevc',
  width: 2560,
  height: 1440,
  fps: 120,
  hdr: false,
  bitrate_kbps: 50_000,
  requested_bitrate_kbps: 50_000,
  mean_bitrate_kbps: 47_600,
  mean_fps: 119.4,
  p95_fps: 117.8,
  loss_percent: 0.04,
  end_reason: 'ended',
  end_detail: '',
  end_category: null,
  started_unix_ms: 1_700_000_000_000,
  ended_unix_ms: 1_700_003_720_000,
  duration_ms: 3_720_000,
  ...changes,
});

test('a session says how it ended, in words rather than host codes', () => {
  for (const [reason, label, tone] of [
    ['ended', 'Ended', 'neutral'],
    ['failed', 'Failed', 'danger'],
    ['start_failed', 'Could not start', 'warn'],
    ['interrupted', 'Host stopped', 'warn'],
  ]) {
    assert.equal(reasonLabel(reason), label);
    assert.equal(reasonTone(reason), tone);
  }
  // A reason a newer host adds still reads as an end, not a blank cell.
  assert.equal(reasonLabel('paused_by_host'), 'Ended');
  assert.equal(reasonTone('paused_by_host'), 'neutral');
  assert.equal(categoryLabel('encoder'), 'Encoder');
  assert.equal(categoryLabel('driver'), 'Driver');
  assert.equal(categoryLabel(null), '');
});

test('a session shows what it ran: codec, picture, rate and HDR', () => {
  assert.equal(codecName('h264'), 'H.264');
  assert.equal(codecName('hevc'), 'HEVC');
  assert.equal(codecName('av1'), 'AV1');
  assert.equal(codecName('pyrowave'), 'PyroWave');
  assert.equal(codecName(''), 'Unknown video format');
  assert.equal(pictureText(entry()), 'HEVC · 2560×1440 · 120 fps');
  assert.equal(pictureText(entry({ hdr: true })), 'HEVC · 2560×1440 · HDR · 120 fps');
  assert.equal(pictureText(entry({ width: 0, height: 0, codec: '', fps: 0 })), 'Unknown video format');
  assert.equal(fpsText(119.44), '119.4 fps');
  assert.equal(fpsText(0), '–');
});

test('bitrate, loss and duration read as measured, not as the request', () => {
  assert.equal(bitrateText(entry()), '47.6 Mbps');
  // A start that never sent a byte has no rate, not the negotiated one.
  assert.equal(bitrateText(entry({ mean_bitrate_kbps: 0, bitrate_kbps: 0 })), '–');
  assert.equal(
    bitrateText(entry({ mean_bitrate_kbps: 0, bitrate_kbps: 20_000 })),
    '20.0 Mbps of 50.0 Mbps'
  );
  // The network held the encoder below the rate the stream asked for: both
  // figures are shown, and an entry saved before the request was recorded
  // still reads as its single rate.
  assert.equal(
    bitrateText(entry({ bitrate_kbps: 40_000, requested_bitrate_kbps: 80_000 })),
    '47.6 Mbps of 80.0 Mbps'
  );
  assert.equal(
    bitrateText(entry({ mean_bitrate_kbps: 0, bitrate_kbps: 40_000, requested_bitrate_kbps: 80_000 })),
    '40.0 Mbps of 80.0 Mbps'
  );
  assert.equal(
    bitrateText(entry({ bitrate_kbps: 40_000, requested_bitrate_kbps: 0 })),
    '47.6 Mbps'
  );
  assert.equal(lossText(entry()), '0.04%');
  assert.equal(lossText(entry({ loss_percent: 1.84 })), '1.8%');
  assert.equal(lossText(entry({ loss_percent: 0 })), '0.0%');
  // A client that reported no FEC blocks did not report "no loss".
  assert.equal(lossText(entry({ loss_percent: null })), '–');
  assert.equal(durationText(entry()), '1 h 2 min');
  assert.equal(durationText(entry({ duration_ms: 45_000 })), '45 s');
  assert.equal(durationText(entry({ duration_ms: 0 })), '–');
});

test('p95 shows only where the host has a 5% tail to report', () => {
  assert.equal(p95Text(117.8), '117.8');
  // Fewer than 20 sampled seconds: the lead-in shows no figure.
  assert.equal(p95Text(0), '–');
  // A start that never ran carries no picture either.
  assert.equal(pictureText(entry({ width: 0, height: 0, fps: 0 })), 'HEVC');
});

test('the search finds a device, a game, a codec or a reason, in any case', () => {
  const session = entry({ end_reason: 'failed', end_category: 'encoder' });
  assert.ok(matches(session, ''));
  assert.ok(matches(session, '  '));
  assert.ok(matches(session, 'pixel'));
  assert.ok(matches(session, 'PORTAL'));
  assert.ok(matches(session, 'hevc'));
  assert.ok(matches(session, 'failed'));
  assert.ok(matches(session, 'encoder'));
  assert.ok(!matches(session, 'xbox'));
});

test('Sessions is a page of its own under Advanced, polling the live streams every 2 s', () => {
  const nav = source('lib/navigation.ts');
  const advanced = nav.slice(nav.indexOf('export const advanced'));
  assert.match(advanced, /path: '\/sessions',\s*label: 'Sessions'/);
  const app = source('App.svelte');
  assert.match(app, /import Sessions from '\.\/pages\/Sessions\.svelte';/);
  assert.match(app, /route\.path === '\/sessions'\) return Sessions;/);
});

test('the page reads the history once and the live streams through the shared poll', () => {
  const page = source('pages/Sessions.svelte');
  // The history poll is also the first load; both stop with the page.
  assert.match(page, /poll\(loadHistory, 15_000\)/);
  assert.match(page, /poll\(refreshStreams, 2000\)/);
  assert.match(page, /api\.sessions\.history\(\)/);
  assert.match(page, /api\.sessions\.streams\(\)/);
  // The caps come from the host's reply, so the subtitle cannot drift from the store.
  assert.match(page, /up to the last \$\{caps\.limit\}, kept for \$\{caps\.days\} days/);
  // A newer response must not be overwritten by an older one.
  assert.match(page, /const id = \+\+latest;[\s\S]*?if \(id !== latest\) return;/);
  // The history table explains its two unusual figures.
  assert.match(page, /p95 is the frame rate the slowest 5% of sampled seconds ran at or below/);
  assert.match(page, /Loss is what the\s+device reported missing/);
  assert.match(page, /p95 \{p95Text\(entry\.p95_fps\)\}/);
});

test('a stream that ends while the page is open reloads the history at once', () => {
  const stream = (uuid, changes = {}) => ({ uuid, state: 'RUNNING', ...changes });
  assert.equal(aStreamEnded(null, [stream('phone')]), false);
  assert.equal(aStreamEnded([], [stream('phone')]), false);
  assert.equal(aStreamEnded([stream('phone')], [stream('phone')]), false);
  // A new stream is not an ended one.
  assert.equal(aStreamEnded([stream('phone')], [stream('phone'), stream('tv')]), false);
  // The stream the page showed is gone.
  assert.equal(aStreamEnded([stream('phone')], []), true);
  assert.equal(aStreamEnded([stream('phone'), stream('tv')], [stream('tv')]), true);
  // A stopping stream is still the row the page shows: nothing reloads yet.
  assert.equal(aStreamEnded([stream('phone')], [stream('phone', { state: 'STOPPING' })]), false);
  assert.equal(aStreamEnded([stream('phone'), stream('tv')], [stream('phone')]), true);
  const page = source('pages/Sessions.svelte');
  assert.match(page, /const ended = aStreamEnded\(streams, next\);/);
  assert.match(page, /if \(ended\) void marked\(loadHistory\);/);
  // The slower tick covers a history write that lands after the immediate one.
  assert.match(page, /stopHistory\(\);/);
  assert.match(page, /stopStreams\(\);/);
});

test('Sessions draws its title, live section and search box without a host', () => {
  const out = render(Sessions, { props: {} }).body;
  assert.match(out, /<h1[^>]*>Sessions<\/h1>/);
  assert.match(out, /Live now/);
  assert.match(out, /Loading the live streams/);
  assert.match(out, />Search sessions</);
  assert.match(out, /placeholder="Search a device or game"/);
});
