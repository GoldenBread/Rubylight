import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// The console's Back and Forward buttons against a "leave this page?" guard. A small fake browser
// keeps the history entries; a traversal (Back, Forward, history.go) is queued and delivered by
// `deliver()`, as a real browser delivers it later, so the order of events is under the test's control.

const server = await createTestServer();
const wait = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A fresh router module over a fake history that holds `paths`, positioned at `at`. */
async function browser(paths, at) {
  const entries = paths.map((path, i) => ({ url: path, state: { rubylightIndex: i } }));
  let cursor = at;
  const queue = [];
  const window = new EventTarget();
  window.scrollTo = () => {};
  const here = () => entries[cursor];
  globalThis.window = window;
  globalThis.document = { getElementById: () => null, querySelectorAll: () => [] };
  globalThis.matchMedia = () => ({ matches: true });
  Object.defineProperty(globalThis, 'location', {
    configurable: true,
    get: () => new URL(here().url, 'http://localhost'),
  });
  globalThis.history = {
    get state() {
      return here().state;
    },
    pushState(state, _title, url) {
      entries.splice(cursor + 1, entries.length, { url, state });
      cursor++;
    },
    replaceState(state, _title, url) {
      entries[cursor] = { url: url ?? here().url, state };
    },
    go(delta) {
      queue.push(delta);
    },
  };
  server.moduleGraph.invalidateAll();
  const router = await server.ssrLoadModule('/src/lib/router.svelte.ts');
  const popstate = () => {
    const event = new Event('popstate');
    event.state = here().state;
    window.dispatchEvent(event);
  };
  const api = {
    router,
    /** The user presses Back / Forward: queued like any traversal. */
    back: () => queue.push(-1),
    forward: () => queue.push(1),
    /** The browser carries out the oldest queued traversal. */
    deliver() {
      const next = cursor + queue.shift();
      if (next < 0 || next >= entries.length) return;
      cursor = next;
      popstate();
    },
    async flush() {
      await wait();
      while (queue.length) {
        api.deliver();
        await wait();
      }
    },
    address: () => location.pathname,
    shown: () => router.route.path,
    entries: () => entries.map((entry) => entry.url),
  };
  return api;
}

/** A guard that stays open until the test answers; counts how often it was asked. */
function dialog(router) {
  const asked = [];
  let answer;
  router.guardNavigation((to) => {
    asked.push(to);
    return new Promise((resolve) => (answer = resolve));
  });
  return {
    asked,
    answer: async (leave) => {
      answer(leave);
      await wait();
    },
  };
}

const inStep = (b) => assert.equal(b.address(), b.shown(), 'the address names the page that is shown');

test('Back with the dialog open, then stay: the page and the address stay where they were', async () => {
  const b = await browser(['/a', '/b', '/c'], 2);
  const d = dialog(b.router);
  b.back();
  b.deliver();
  await wait();
  assert.deepEqual(d.asked, ['/b']);
  await d.answer(false);
  await b.flush();
  assert.equal(b.shown(), '/c');
  inStep(b);
  assert.deepEqual(d.asked, ['/b']);
});

test('Back with the dialog open, then leave: the page follows the address', async () => {
  const b = await browser(['/a', '/b', '/c'], 2);
  const d = dialog(b.router);
  b.back();
  b.deliver();
  await wait();
  await d.answer(true);
  await b.flush();
  assert.equal(b.shown(), '/b');
  inStep(b);
  assert.equal(d.asked.length, 1);
});

test('Back twice quickly asks once; stay returns to the page shown, leave follows the last Back', async () => {
  for (const leave of [false, true]) {
    const b = await browser(['/a', '/b', '/c'], 2);
    const d = dialog(b.router);
    b.back();
    b.back();
    b.deliver();
    await wait();
    b.deliver();
    await wait();
    await d.answer(leave);
    await b.flush();
    assert.equal(b.shown(), leave ? '/a' : '/c');
    inStep(b);
    assert.equal(d.asked.length, 1);
  }
});

test('Forward after stay asks once about the next page and follows the answer', async () => {
  for (const leave of [false, true]) {
    const b = await browser(['/a', '/b', '/c', '/d'], 2);
    const d = dialog(b.router);
    b.back();
    b.deliver();
    await wait();
    await d.answer(false);
    await b.flush();
    b.forward();
    await b.flush();
    assert.deepEqual(d.asked, ['/b', '/d']);
    await d.answer(leave);
    await b.flush();
    assert.equal(b.shown(), leave ? '/d' : '/c');
    inStep(b);
    assert.equal(d.asked.length, 2);
  }
});

test('Back pressed while a link opened the dialog: stay returns to the page shown', async () => {
  const b = await browser(['/a', '/b', '/c'], 2);
  const d = dialog(b.router);
  b.router.navigate('/z');
  await wait();
  b.back();
  b.deliver();
  await wait();
  assert.deepEqual(d.asked, ['/z']);
  await d.answer(false);
  await b.flush();
  assert.equal(b.shown(), '/c');
  inStep(b);
  assert.equal(d.asked.length, 1);
});

test('Back pressed while a link opened the dialog: leave follows the Back, not the link', async () => {
  const b = await browser(['/a', '/b', '/c'], 2);
  const d = dialog(b.router);
  b.router.navigate('/z');
  await wait();
  b.back();
  b.deliver();
  await wait();
  await d.answer(true);
  await b.flush();
  assert.equal(b.shown(), '/b');
  inStep(b);
  assert.deepEqual(b.entries(), ['/a', '/b', '/c']);
});

test('a guard that fails counts as stay and does not block the next Back', async () => {
  const b = await browser(['/a', '/b', '/c'], 2);
  let calls = 0;
  b.router.guardNavigation(() => {
    calls++;
    return calls === 1 ? Promise.reject(new Error('boom')) : Promise.resolve(true);
  });
  b.back();
  b.deliver();
  await b.flush();
  assert.equal(b.shown(), '/c');
  inStep(b);
  b.back();
  await b.flush();
  assert.equal(b.shown(), '/b');
  inStep(b);
});

test('a leave answered with no guard in the way keeps working: Back moves the page', async () => {
  const b = await browser(['/a', '/b', '/c'], 2);
  b.back();
  await b.flush();
  assert.equal(b.shown(), '/b');
  inStep(b);
});
