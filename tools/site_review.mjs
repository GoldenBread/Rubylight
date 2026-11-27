// Render and audit both static sites (_site/ and client/site/) across 360, 768, 1440 px in Light & Dark mode.
// Binds strictly to 127.0.0.1:0 (zero firewall prompts) and asserts zero horizontal page overflow.
//
// Usage: node tools/site_review.mjs <out-dir>

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(__dirname, '..');
const outDir = path.resolve(process.argv[2] || path.join(repo, '_site_review'));

const requireFromWeb = createRequire(path.join(repo, 'rust/web/package.json'));
let playwrightMod;
try {
  playwrightMod = requireFromWeb(process.env.PLAYWRIGHT_MODULE || 'playwright');
} catch {
  const requireFromDemo = createRequire(path.join(repo, 'client/scripts/demo/package.json'));
  playwrightMod = requireFromDemo('playwright-core');
}
const { chromium } = playwrightMod;

function findChromiumExecutable() {
  if (process.env.CHROMIUM_EXECUTABLE && fs.existsSync(process.env.CHROMIUM_EXECUTABLE)) {
    return process.env.CHROMIUM_EXECUTABLE;
  }
  const localAppData = process.env.LOCALAPPDATA || '';
  const msPlaywright = path.join(localAppData, 'ms-playwright');
  if (fs.existsSync(msPlaywright)) {
    const dirs = fs.readdirSync(msPlaywright).filter((d) => d.startsWith('chromium-')).sort().reverse();
    for (const d of dirs) {
      const cand = path.join(msPlaywright, d, 'chrome-win64', 'chrome.exe');
      if (fs.existsSync(cand)) return cand;
    }
  }
  return undefined;
}

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.webp': 'image/webp',
  '.mp4': 'video/mp4',
  '.ttf': 'font/ttf',
};

function serveStatic(rootDir) {
  return new Promise((resolvePromise) => {
    const server = http.createServer((req, res) => {
      const reqPath = decodeURIComponent((req.url || '/').split('?')[0]);
      let rel = reqPath.replace(/^\/+/, '');
      if (!rel || rel.endsWith('/')) rel += 'index.html';
      const target = path.resolve(rootDir, rel);
      if (!target.startsWith(path.resolve(rootDir)) || !fs.existsSync(target) || !fs.statSync(target).isFile()) {
        res.writeHead(404);
        res.end('Not found');
        return;
      }
      const ext = path.extname(target).toLowerCase();
      res.writeHead(200, { 'Content-Type': MIME[ext] || 'application/octet-stream' });
      fs.createReadStream(target).pipe(res);
    });
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      resolvePromise({
        base: `http://127.0.0.1:${addr.port}`,
        close: () => new Promise((r) => server.close(r)),
      });
    });
  });
}

const VIEWPORTS = [
  { width: 1440, height: 960 },
  { width: 768, height: 1024 },
  { width: 360, height: 800 },
];

async function auditSite(browser, label, rootDir, pages) {
  const srv = await serveStatic(rootDir);
  const failures = [];
  try {
    const siteOut = path.join(outDir, label);
    fs.mkdirSync(siteOut, { recursive: true });
    for (const theme of ['dark', 'light']) {
      for (const vp of VIEWPORTS) {
        const ctx = await browser.newContext({ viewport: vp, colorScheme: theme, deviceScaleFactor: 1 });
        const page = await ctx.newPage();
        for (const [slug, route] of pages) {
          await page.goto(`${srv.base}/${route}`, { waitUntil: 'networkidle' });
          await page.evaluate(() => document.fonts && document.fonts.ready);
          const overflow = await page.evaluate(() => ({
            scrollW: document.documentElement.scrollWidth,
            clientW: document.documentElement.clientWidth,
          }));
          if (overflow.scrollW > overflow.clientW + 1) {
            failures.push(`${label} ${route} (${theme} ${vp.width}px): horizontal overflow ${overflow.scrollW} > ${overflow.clientW}`);
          }
          const shotPath = path.join(siteOut, `${slug}-${theme}-${vp.width}.png`);
          await page.screenshot({ path: shotPath, fullPage: false });
        }
        await ctx.close();
      }
    }
  } finally {
    await srv.close();
  }
  return failures;
}

async function main() {
  fs.mkdirSync(outDir, { recursive: true });
  const executablePath = findChromiumExecutable();
  const browser = await chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
  let failures = [];
  try {
    if (fs.existsSync(path.join(repo, '_site/index.html'))) {
      failures = failures.concat(
        await auditSite(browser, 'rubylight', path.join(repo, '_site'), [
          ['home', 'index.html'],
          ['getting-started', 'docs/getting-started.html'],
        ]),
      );
    }
    failures = failures.concat(
      await auditSite(browser, 'nightjar', path.join(repo, 'client/site'), [
        ['home', 'index.html'],
        ['home-de', 'de/index.html'],
        ['download', 'download.html'],
        ['download-de', 'de/download.html'],
        ['docs', 'docs.html'],
        ['devices', 'devices.html'],
        ['privacy', 'privacy.html'],
        ['404', '404.html'],
      ]),
    );
  } finally {
    await browser.close();
  }
  if (failures.length) {
    console.error('Site visual audit failures:\n  ' + failures.join('\n  '));
    process.exit(1);
  }
  console.log(`PASS: zero horizontal overflow across all viewports/themes; screenshots in ${outDir}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
