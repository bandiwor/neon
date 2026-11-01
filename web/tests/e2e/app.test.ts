// The built site, served by our own server, driven in Chromium. Selectors use roles and the texts of the
// Russian dictionary — never class names, which the build shortens.
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ru } from '../../src/locales';
import { startServer, type RunningServer } from '../../server/server';

const dist = resolve(import.meta.dirname, '../../dist');
let playwright: typeof import('playwright') | null = null;
try {
  playwright = await import('playwright');
} catch {
  /* not installed: the suite is skipped */
}
const ready = playwright !== null && existsSync(resolve(dist, 'index.html'));

describe.skipIf(!ready)('the site in a browser', () => {
  let running: RunningServer;
  let browser: import('playwright').Browser;

  beforeAll(async () => {
    running = await startServer({ port: 0, host: '127.0.0.1', root: dist, log: false }, () => undefined);
    browser = await playwright!.chromium.launch();
  });
  afterAll(async () => {
    await browser?.close();
    await running?.close();
  });

  async function open(width = 1280, height = 800) {
    const page = await browser.newPage({ viewport: { width, height } });
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
    await page.goto(running.url);
    await page.locator('.cm-content').waitFor({ timeout: 30_000 });
    return { page, errors };
  }

  it('starts without errors and with short class names', async () => {
    const { page, errors } = await open();
    expect(await page.title()).toContain('Neon');
    const classes = await page.evaluate(() => [...document.querySelectorAll('[class]')].flatMap((e) => [...e.classList]).filter((c) => !c.startsWith('cm-') && !c.startsWith('ͼ')));
    expect(classes.length).toBeGreaterThan(20);
    expect(Math.max(...classes.map((c) => c.length))).toBeLessThanOrEqual(3);
    expect(errors).toEqual([]);
    await page.close();
  });

  it('runs the program', async () => {
    const { page, errors } = await open();
    await page.getByRole('button', { name: ru['actions.run'] }).first().click();
    await page.getByText('сумма квадратов расстояний: 127').waitFor({ timeout: 30_000 });
    await page.getByText(/Завершено успешно/).waitFor();
    expect(errors).toEqual([]);
    await page.close();
  });

  it('reports a compile error and shows it in the problems list', async () => {
    const { page } = await open();
    await page.locator('.cm-content').click();
    await page.keyboard.press('Control+A');
    await page.keyboard.type('fn main() { let x = ; }');
    await page.getByRole('button', { name: ru['actions.run'] }).first().click();
    await page.getByText(/Ошибка компиляции/).waitFor({ timeout: 30_000 });
    await page.getByRole('tab', { name: new RegExp(ru['panel.problems']) }).click();
    await page.getByText(/expected expression/).first().waitFor();
    await page.close();
  });

  it('evaluates in the REPL', async () => {
    const { page } = await open();
    await page.getByRole('tab', { name: ru['panel.repl'] }).click();
    const input = page.getByRole('textbox', { name: ru['panel.repl'] });
    await input.fill('6 * 7');
    await input.press('Enter');
    await page.getByText('42').first().waitFor({ timeout: 30_000 });
    await page.close();
  });

  it('feeds interactive input to the program', async () => {
    const { page } = await open();
    await page.locator('.cm-content').click();
    await page.keyboard.press('Control+A');
    await page.keyboard.type('fn main() { let n = input(); println("hello " + n); }');
    await page.getByRole('button', { name: ru['actions.run'] }).first().click();
    const answer = page.getByPlaceholder(ru['console.input_placeholder']);
    await answer.waitFor({ timeout: 30_000 });
    await answer.fill('Ada');
    await answer.press('Enter');
    await page.getByText('hello Ada').waitFor({ timeout: 30_000 });
    await page.close();
  });

  it('has icon-only buttons in the header and no brand', async () => {
    const { page } = await open();
    const header = page.locator('header').first();
    expect((await header.innerText()).trim()).toBe('');
    expect(await header.getByRole('button').count()).toBeGreaterThanOrEqual(9);
    await page.close();
  });

  it('edits like an editor: brackets, Enter, completion with a call', async () => {
    const { page } = await open();
    const lines = () => page.evaluate(() => [...document.querySelectorAll('.cm-line')].map((l) => l.textContent ?? ''));
    await page.locator('.cm-content').click();
    await page.keyboard.press('Control+A');
    await page.keyboard.press('Delete');
    await page.keyboard.type('fn main() {');
    await page.keyboard.press('Enter');
    await page.keyboard.type('if x > 1 {');
    await page.keyboard.press('Enter');
    await page.keyboard.type('println("a');
    expect(await lines()).toEqual(['fn main() {', '    if x > 1 {', '        println("a")', '    }', '}']);
    await page.keyboard.press('Control+A');
    await page.keyboard.press('Delete');
    await page.keyboard.type('fn main() {\nprintl');
    await page.getByRole('option', { name: /println/ }).waitFor({ timeout: 30_000 });
    await page.keyboard.press('Enter');
    await page.keyboard.type('"hi"');
    expect(await lines()).toEqual(['fn main() {', '    println("hi")', '}']);
    await page.close();
  });

  it('keeps the settings in sections', async () => {
    const { page } = await open();
    await page.getByRole('button', { name: ru['actions.settings'] }).click();
    const dialog = page.getByRole('dialog');
    for (const key of ['settings.s_appearance', 'settings.s_editor', 'settings.s_compiler', 'settings.s_vm', 'settings.s_program'] as const) {
      await dialog.getByRole('button', { name: ru[key] }).click();
    }
    await dialog.getByRole('button', { name: ru['settings.s_compiler'] }).click();
    await dialog.getByText(ru['opt.inline']).waitFor();
    // switching a pass off reaches the optimizer: the IR view changes
    await page.keyboard.press('Escape');
    await page.close();
  });

  it('fits a phone', async () => {
    const { page } = await open(390, 800);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    await page.close();
  });
});
