// The page in a real browser (needs Playwright and a built dist/; skipped otherwise).
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
let playwright = null;
try {
  playwright = require('playwright');
} catch {
  /* not installed */
}
const built = existsSync(new URL('../dist/index.html', import.meta.url)) && existsSync(new URL('../dist/neon.wasm', import.meta.url));
const skip = (!playwright && 'playwright is not installed') || (!built && 'dist/ is not built');

let server;
let browser;
const port = 18000 + Math.floor(Math.random() * 1000);
const url = `http://localhost:${port}/`;

before(async () => {
  if (skip) return;
  server = spawn('node', ['serve.mjs', String(port)], { cwd: new URL('..', import.meta.url).pathname, stdio: 'ignore' });
  await new Promise((r) => setTimeout(r, 600));
  browser = await playwright.chromium.launch();
});
after(async () => {
  await browser?.close();
  server?.kill();
});

async function open(opts = {}) {
  const ctx = await browser.newContext({ viewport: { width: 1360, height: 800 }, ...opts });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(url);
  await page.waitForSelector('.cm-content');
  await page.waitForSelector('.lsp-ready', { timeout: 20000 });
  return { page, errors, ctx };
}

test('runs the starting program and shows its output', { skip }, async () => {
  const { page, errors, ctx } = await open();
  await page.keyboard.press('Control+Enter');
  await page.waitForSelector('.statusline.status-ok', { timeout: 20000 });
  assert.match(await page.locator('.console-out').innerText(), /сумма квадратов расстояний: 127/);
  assert.deepEqual(errors, []);
  await ctx.close();
});

test('reports a type error as a problem and as a squiggle', { skip }, async () => {
  const { page, ctx } = await open();
  await page.click('.cm-content');
  await page.keyboard.press('Control+End');
  await page.keyboard.type('\nlet bad: i64 = "text";\n');
  await page.waitForSelector('.cm-lintRange-error', { timeout: 10000 });
  await page.click('.panel .tab:has-text("Проблемы")');
  assert.match(await page.locator('.problems').innerText(), /mismatched types/);
  await ctx.close();
});

test('an interactive program asks for input in the console', { skip }, async () => {
  const { page, ctx } = await open();
  await page.click('.sidebar .tab[title="Примеры"]');
  await page.click('text=Рисовалка');
  await page.keyboard.press('Control+Enter');
  await page.waitForSelector('.input-row input', { timeout: 20000 });
  await page.fill('.input-row input', 'ddd');
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => /pos \(27, 9\)/.test(document.querySelector('.console-out').innerText), null, { timeout: 20000 });
  await ctx.close();
});

test('the REPL keeps its cells and previews the value', { skip }, async () => {
  const { page, ctx } = await open();
  await page.click('.panel .tab:has-text("REPL")');
  await page.click('.repl-field textarea');
  await page.keyboard.type('let n = 20;');
  await page.keyboard.press('Enter');
  await page.keyboard.type('n * 2 + 2');
  await page.waitForSelector('.repl-preview:has-text("42")', { timeout: 10000 });
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => /\n42/.test(document.querySelector('.repl-log').innerText));
  await ctx.close();
});

test('the theme and the accent follow the settings, in both schemes', { skip }, async () => {
  const { page, ctx } = await open({ colorScheme: 'dark' });
  const bg = () => page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  const dark = await bg();
  await page.click('button[title^="Настройки"]');
  await page.click('.segmented button:has-text("Светлая")');
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light');
  const light = await bg();
  assert.notEqual(dark, light);
  await page.click('.swatch:has-text("Лазурь")');
  await page.waitForFunction(() => document.documentElement.dataset.accent === 'azure');
  await ctx.close();
});

test('a shared link restores the project', { skip }, async () => {
  const { page, ctx } = await open();
  await page.click('.cm-content');
  await page.keyboard.press('Control+a');
  await page.keyboard.type('fn main() { println("from a link"); }');
  await page.waitForTimeout(500);
  await ctx.grantPermissions(['clipboard-read', 'clipboard-write']).catch(() => {});
  await page.click('button:has-text("Поделиться")');
  await page.waitForTimeout(800);
  const hash = await page.evaluate(() => location.hash);
  assert.match(hash, /^#p=/);
  const second = await browser.newContext({ viewport: { width: 1360, height: 800 } });
  const other = await second.newPage();
  await other.goto(url + hash);
  await other.waitForSelector('.cm-content');
  assert.match(await other.locator('.cm-content').innerText(), /from a link/);
  await ctx.close();
  await second.close();
});

test('a second file is created, imported and run', { skip }, async () => {
  const { page, ctx } = await open();
  await page.click('button[title="Новый файл"]');
  const input = page.locator('.inline-input');
  await input.waitFor();
  await input.fill('lib/util.neon');
  await input.press('Enter');
  await page.waitForSelector('.filetab:has-text("lib/util.neon")');
  await page.click('.cm-content');
  await page.keyboard.type('export fn triple(n: i64) -> i64 { n * 3 }');
  await page.click('.filetab:has-text("main.neon")');
  await page.click('.cm-content');
  await page.keyboard.press('Control+a');
  await page.keyboard.type('import { triple } from "lib/util";\nfn main() { println(triple(14)); }');
  await page.keyboard.press('Control+Enter');
  await page.waitForSelector('.statusline.status-ok', { timeout: 20000 });
  assert.match(await page.locator('.console-out').innerText(), /42/);
  await ctx.close();
});

test('rename changes every use and go-to-definition finds the declaration', { skip }, async () => {
  const { page, ctx } = await open();
  await page.click('.cm-content');
  await page.keyboard.press('Control+a');
  await page.keyboard.type('fn answer() -> i64 { 42 }\nfn main() { println(answer()); }');
  await page.waitForTimeout(700);
  // ctrl+click on the use
  await page.locator('.cm-line', { hasText: 'println' }).locator('span', { hasText: 'answer' }).first().click({ modifiers: ['Control'] });
  await page.waitForFunction(() => /Стр 1,/.test(document.querySelector('.statusbar').innerText));
  await page.keyboard.press('F2');
  await page.waitForSelector('.dialog input');
  await page.fill('.dialog input', 'reply');
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => (document.querySelector('.cm-content').innerText.match(/reply/g) ?? []).length === 2);
  await ctx.close();
});
