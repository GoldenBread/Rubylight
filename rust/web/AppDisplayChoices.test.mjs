import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const { AppDraft, ALWAYS } = await server.ssrLoadModule('/src/components/library/draft.svelte.ts');
const { default: DisplaySettings } = await server.ssrLoadModule('/src/components/library/DisplaySettings.svelte');
const { default: VideoSettings } = await server.ssrLoadModule('/src/components/library/VideoSettings.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const app = extra => ({ name: 'Game', cmd: 'game.exe', 'working-dir': '', 'prep-cmd': [], uuid: 'a', ...extra });
const display = draft => render(DisplaySettings, { props: { draft } }).body;
const video = draft => render(VideoSettings, { props: { draft } }).body;

test('the virtual display mode holds the Always choice that was a switch of its own', () => {
  // display_session.rs: virtual-display (or Apollo's virtual-screen) asks for one whatever the mode.
  assert.equal(new AppDraft(app({})).virtualDisplay(), '');
  assert.equal(new AppDraft(app({ 'virtual-display-mode': 'shared' })).virtualDisplay(), 'shared');
  assert.equal(new AppDraft(app({ 'virtual-display': true })).virtualDisplay(), ALWAYS);
  assert.equal(new AppDraft(app({ 'virtual-screen': true })).virtualDisplay(), ALWAYS);
  assert.equal(new AppDraft(app({ 'virtual-display': 'false', 'virtual-screen': 'true' })).virtualDisplay(), ALWAYS);
  assert.equal(new AppDraft(app({ 'virtual-display': true, 'virtual-display-mode': 'shared' })).virtualDisplay(), `${ALWAYS}:shared`);

  const draft = new AppDraft(app({ 'virtual-screen': true, 'virtual-display-mode': 'per_client' }));
  draft.setVirtualDisplay(ALWAYS);
  let record = draft.record();
  assert.equal(record['virtual-display'], true);
  assert.ok(!('virtual-screen' in record));
  assert.ok(!('virtual-display-mode' in record));
  draft.setVirtualDisplay('shared');
  record = draft.record();
  assert.ok(!('virtual-display' in record));
  assert.equal(record['virtual-display-mode'], 'shared');
  draft.setVirtualDisplay(`${ALWAYS}:shared`);
  record = draft.record();
  assert.equal(record['virtual-display'], true);
  assert.equal(record['virtual-display-mode'], 'shared');
  draft.setVirtualDisplay('');
  record = draft.record();
  assert.ok(!('virtual-display' in record));
  assert.ok(!('virtual-display-mode' in record));
});

test('the editor shows one virtual display choice and no separate switch', () => {
  const html = display(new AppDraft(app({ 'virtual-display': true })));
  assert.doesNotMatch(html, /Use a virtual display/);
  assert.match(html, /<option value="always"[^>]*selected/);
  assert.match(html, /Always, even where a monitor is set/);
  // A mode saved beside Always stays selectable as it is.
  const kept = display(new AppDraft(app({ 'virtual-display': true, 'virtual-display-mode': 'shared' })));
  assert.match(kept, /<option value="always:shared"[^>]*selected[^>]*>Always, one shared by all devices/);
  assert.doesNotMatch(display(new AppDraft(app({}))), /value="always:/);
});

test('display switches say when another choice takes their place', () => {
  const toggle = (html, label) => html.match(new RegExp(`<label class="toggle[^"]*"[^>]*>(?:(?!</label>).)*${label}`, 's'))?.[0] ?? '';
  const free = display(new AppDraft(app({ 'use-app-identity': true })));
  assert.doesNotMatch(toggle(free, 'Make the virtual display primary'), /disabled/);
  assert.doesNotMatch(toggle(free, 'Keep a separate identity for each device'), /disabled/);
  // The app's own layout decides the primary screen, and the per-device identity needs the app identity.
  const set = display(new AppDraft(app({ 'virtual-display-layout': 'exclusive' })));
  assert.match(toggle(set, 'Make the virtual display primary'), /disabled/);
  assert.match(toggle(set, 'Keep a separate identity for each device'), /disabled/);
  assert.match(set, /Not used: Virtual display layout below is set/);
  assert.match(set, /Not used: turn on Keep one display identity for this app first/);
});

test('the legacy frame generation switch is inactive while Frame generation is set, and the inert fix is gone', () => {
  const plain = video(new AppDraft(app({})));
  assert.doesNotMatch(plain, /Frame generation limiter fix/);
  assert.match(plain, /Legacy frame generation capture fix/);
  assert.doesNotMatch(plain, /Not used: Frame generation above is set/);
  // Even None is a value: display_session.rs then ignores the legacy switch.
  const set = video(new AppDraft(app({ 'frame-generation-mode': 'none', 'gen1-framegen-fix': true })));
  assert.match(set, /Not used: Frame generation above is set and takes its place\./);
  // The editor no longer writes the removed switch; the host strips a saved one.
  assert.ok(!('frame-gen-limiter-fix' in new AppDraft(app({})).record()));
});
