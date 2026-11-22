import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { default: LogFindings } = await server.ssrLoadModule('/src/components/maintenance/LogFindings.svelte');
const { default: ClientAgent } = await server.ssrLoadModule('/src/components/devices/ClientAgent.svelte');
const { render } = await server.ssrLoadModule('svelte/server');

const finding = {
  id: 'encoder_hang',
  category: 'encoder',
  level: 'error',
  title: 'The AMD encoder stopped answering',
  explanation: 'An AMF call took seconds.',
  fix: 'Update the AMD driver.',
  link: 'https://github.com/RamazanKara/Rubylight/blob/main/docs/troubleshooting.md#a-driver-is-missing-or-out-of-date',
  count: 3,
  first_seen: '2026-10-06T17:52:03.632096Z',
  last_seen: '2026-10-07T09:00:00.000000Z',
  sample: 'butterpollo::stream: slow encoder call call="AMF SubmitInput" elapsed_ms=34787',
};
const findings = (list, since = '2026-10-06T00:00:00Z') =>
  render(LogFindings, { props: { analysis: { status: true, findings: list, since, until: since, read_bytes: 10 } } }).body;

test('log analysis lists each finding with its count, fix and guide link', () => {
  const html = findings([finding, { ...finding, id: 'roaming', category: 'network', level: 'warn', count: 1, title: 'The client changed networks' }]);
  assert.match(html, /The AMD encoder stopped answering/);
  assert.match(html, /3 times, last /);
  assert.match(html, /Once, last /);
  assert.match(html, /Update the AMD driver\./);
  assert.match(html, /Encoder/);
  assert.match(html, /Network/);
  assert.match(html, /href="https:\/\/github\.com\/RamazanKara\/Rubylight\/blob\/main\/docs\/troubleshooting\.md#a-driver-is-missing-or-out-of-date"/);
  assert.match(html, /target="_blank" rel="noreferrer"/);
  assert.match(html, /elapsed_ms=34787/);
});

test('a fix that names a setting links to its row in All settings', async () => {
  const html = findings([{ ...finding, id: 'capture_stall', setting: 'capture' }]);
  assert.match(html, /href="\/settings\/picture\?setting=capture"[^>]*>Open (?:<!---->)?Capture method/);
  // A key the console doesn't describe gets no link; the guide link stays.
  assert.doesNotMatch(findings([{ ...finding, setting: 'no_such_setting' }]), /Open /);
  assert.doesNotMatch(findings([finding]), /href="\/settings/);
  // Every setting the host's rules link to is one the console shows.
  const { readFile } = await import('node:fs/promises');
  const rules = await readFile(new URL('../core/session/src/log_analysis.rs', import.meta.url), 'utf8');
  const linked = [...rules.matchAll(/setting: Some\("([a-z0-9_]+)"\)/g)].map(match => match[1]);
  assert(linked.length >= 4, `only ${linked.length} links found: the pattern no longer matches the code`);
  const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
  const keys = new Set(schema.settings.map(setting => setting.key));
  assert.deepEqual(linked.filter(key => !keys.has(key)), []);
});

test('log analysis says so when the log has no known problems', () => {
  assert.match(findings([]), /No known problems in the log since /);
  assert.match(findings([], null), /No known problems in the log\./);
});

test('log analysis escapes log text', () => {
  const html = findings([{ ...finding, sample: 'client=<script>alert(1)</script>' }]);
  assert.match(html, /&lt;script/);
  assert.doesNotMatch(html, /<script>alert/);
});

test('the device list names the client and warns only about listed builds', () => {
  const agent = (client) => render(ClientAgent, { props: { client } }).body;
  const plain = agent({ client_agent: 'Nightjar', client_warning: null });
  assert.match(plain, /Client: <span class="mono">Nightjar<\/span>/);
  assert.doesNotMatch(plain, /known problem/);
  assert.doesNotMatch(agent({}), /Client:/);
  const warned = agent({
    client_agent: 'Nightjar/3.1.2 <b>',
    client_warning: { problem: 'freezes after a keyframe request', link: 'https://github.com/RamazanKara/Rubylight/issues/1' },
  });
  assert.match(warned, /known problem: freezes after a keyframe request\. It can still connect\./);
  assert.match(warned, /href="https:\/\/github\.com\/RamazanKara\/Rubylight\/issues\/1"/);
  assert.match(warned, /Nightjar\/3\.1\.2 &lt;b>/);
});
