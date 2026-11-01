// The static server: configuration, path safety, compression, caching and headers, then the whole thing over HTTP.
import { mkdirSync, mkdtempSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { brotliCompressSync, gzipSync } from 'node:zlib';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ConfigError, parseConfig } from '../../server/config';
import { buildCsp, inlineScripts, scriptHash } from '../../server/csp';
import { acceptedEncodings, locate } from '../../server/files';
import { cacheControl, etag, notModified } from '../../server/headers';
import { contentType } from '../../server/mime';
import { startServer, type RunningServer } from '../../server/server';

describe('configuration', () => {
  const parse = (argv: string[], env: Record<string, string> = {}) => parseConfig(argv, env, '/work');

  it('has defaults', () => {
    expect(parse([])).toEqual({ port: 8080, host: 'localhost', root: '/work/dist', log: true });
  });

  it('reads options, a bare port and the environment', () => {
    expect(parse(['-p', '0', '-H', '0.0.0.0', '-d', 'out', '-q'])).toEqual({ port: 0, host: '0.0.0.0', root: '/work/out', log: false });
    expect(parse(['9000']).port).toBe(9000);
    expect(parse([], { PORT: '1234', HOST: 'example' })).toMatchObject({ port: 1234, host: 'example' });
    expect(parse(['--port', '5'], { PORT: '1234' }).port).toBe(5);
  });

  it('rejects bad input', () => {
    for (const argv of [['--nope'], ['-p'], ['-p', 'x'], ['70000'], ['-p', '1.5']]) expect(() => parse(argv), argv.join(' ')).toThrow(ConfigError);
  });
});

describe('mime types, caching and validators', () => {
  it('knows what the site serves', () => {
    expect(contentType('a/index.html')).toMatch(/^text\/html/);
    expect(contentType('x.wasm')).toBe('application/wasm');
    expect(contentType('x.js')).toMatch(/javascript/);
    expect(contentType('x.unknown')).toBe('application/octet-stream');
  });

  it('caches hashed assets forever and revalidates the rest', () => {
    expect(cacheControl('/d/assets/index-AbC123_x.js')).toMatch(/immutable/);
    expect(cacheControl('/d/index.html')).toBe('no-cache');
    expect(cacheControl('/d/assets/plain.js')).toBe('no-cache');
  });

  it('matches ETags', () => {
    const tag = etag({ size: 255, mtimeMs: 4096 });
    expect(tag).toBe('W/"ff-1000"');
    expect(etag({ size: 255, mtimeMs: 4096 }, 'br')).toBe('W/"ff-1000-br"');
    expect(notModified(tag, tag)).toBe(true);
    expect(notModified(`"x", ${tag}`, tag)).toBe(true);
    expect(notModified('*', tag)).toBe(true);
    expect(notModified('"x"', tag)).toBe(false);
    expect(notModified(undefined, tag)).toBe(false);
  });

  it('reads Accept-Encoding', () => {
    expect(acceptedEncodings('gzip, deflate, br')).toEqual(['br', 'gzip']);
    expect(acceptedEncodings('gzip;q=0, br;q=1')).toEqual(['br']);
    expect(acceptedEncodings('identity')).toEqual([]);
    expect(acceptedEncodings(undefined)).toEqual([]);
  });
});

describe('content security policy', () => {
  it('hashes the inline scripts and only those', () => {
    const html = '<script src="a.js"></script><script>let a=1</script><script type="module">b()</script>';
    expect(inlineScripts(html)).toEqual(['let a=1', 'b()']);
    expect(scriptHash('x')).toMatch(/^'sha256-[A-Za-z0-9+/]+=*'$/);
  });
});

describe('serving a site', () => {
  const root = mkdtempSync(join(tmpdir(), 'neon-site-'));
  const index = '<!doctype html><script>window.x=1</script><title>t</title>';
  let running: RunningServer;
  const get = (path: string, headers: Record<string, string> = {}, method = 'GET') =>
    fetch(new URL(path, running.url), { method, headers, redirect: 'manual' });

  beforeAll(async () => {
    mkdirSync(join(root, 'assets'));
    writeFileSync(join(root, 'index.html'), index);
    writeFileSync(join(root, 'assets', 'app-Abcdef12.js'), 'console.log("hi");'.repeat(50));
    writeFileSync(join(root, 'data.json'), '{"a":1}');
    const js = 'console.log("hi");'.repeat(50);
    writeFileSync(join(root, 'assets', 'app-Abcdef12.js.br'), brotliCompressSync(js));
    writeFileSync(join(root, 'assets', 'app-Abcdef12.js.gz'), gzipSync(js));
    writeFileSync(join(root, 'secret.txt'), 'outside');
    mkdirSync(join(root, 'public'));
    writeFileSync(join(root, 'public', 'a.txt'), 'inside');
    running = await startServer({ port: 0, host: '127.0.0.1', root: join(root, 'public'), log: false }, () => undefined);
  });
  afterAll(async () => {
    await running.close();
    rmSync(root, { recursive: true, force: true });
  });

  it('serves files from the root and nothing above it', async () => {
    expect(await (await get('/a.txt')).text()).toBe('inside');
    for (const path of ['/../secret.txt', '/%2e%2e/secret.txt', '/..%2fsecret.txt', '/%00', '/%zz', '/nothing']) {
      expect((await get(path)).status, path).toBe(404);
    }
    expect(await locate(join(root, 'public'), '/../secret.txt')).toBeNull();
  });

  it('refuses methods other than GET and HEAD', async () => {
    const r = await get('/a.txt', {}, 'POST');
    expect([r.status, r.headers.get('allow')]).toEqual([405, 'GET, HEAD']);
    const head = await get('/a.txt', {}, 'HEAD');
    expect([head.status, await head.text(), head.headers.get('content-length')]).toEqual([200, '', '6']);
  });

  it('sends security headers', async () => {
    const r = await get('/a.txt');
    expect(r.headers.get('x-content-type-options')).toBe('nosniff');
    expect(r.headers.get('x-frame-options')).toBe('DENY');
  });

  it('answers a repeated request with 304', async () => {
    const first = await get('/a.txt');
    const tag = first.headers.get('etag') ?? '';
    expect(tag).toMatch(/^W\//);
    const again = await get('/a.txt', { 'if-none-match': tag });
    expect([again.status, await again.text()]).toEqual([304, '']);
  });
});

describe('serving the built site', () => {
  const root = mkdtempSync(join(tmpdir(), 'neon-built-'));
  const js = 'console.log("hi");'.repeat(50);
  const index = '<!doctype html><script>window.x=1</script>';
  let running: RunningServer;
  const get = (path: string, headers: Record<string, string> = {}) => fetch(new URL(path, running.url), { headers });

  beforeAll(async () => {
    mkdirSync(join(root, 'assets'));
    writeFileSync(join(root, 'index.html'), index);
    const file = join(root, 'assets', 'app-Abcdef12.js');
    writeFileSync(file, js);
    writeFileSync(`${file}.br`, brotliCompressSync(js));
    writeFileSync(`${file}.gz`, gzipSync(js));
    running = await startServer({ port: 0, host: '127.0.0.1', root, log: false }, () => undefined);
  });
  afterAll(async () => {
    await running.close();
    rmSync(root, { recursive: true, force: true });
  });

  it('serves the index for a directory with a policy that allows its inline script', async () => {
    const r = await get('/');
    expect(r.headers.get('content-type')).toMatch(/text\/html/);
    expect(r.headers.get('cache-control')).toBe('no-cache');
    expect(await r.text()).toBe(index);
    const csp = r.headers.get('content-security-policy') ?? '';
    expect(csp).toContain(scriptHash('window.x=1'));
    expect(csp).toContain("'wasm-unsafe-eval'");
    expect(csp).toBe(await buildCsp(root));
  });

  it('negotiates the best precompressed copy', async () => {
    const path = '/assets/app-Abcdef12.js';
    const br = await get(path, { 'accept-encoding': 'gzip, br' });
    expect([br.headers.get('content-encoding'), await br.text()]).toEqual(['br', js]);
    const gz = await get(path, { 'accept-encoding': 'gzip' });
    expect([gz.headers.get('content-encoding'), await gz.text()]).toEqual(['gzip', js]);
    const plain = await get(path, { 'accept-encoding': 'identity' });
    expect([plain.headers.get('content-encoding'), await plain.text()]).toEqual([null, js]);
    expect(br.headers.get('cache-control')).toMatch(/immutable/);
    expect(br.headers.get('vary')).toBe('Accept-Encoding');
    expect(br.headers.get('content-type')).toMatch(/javascript/);
  });

  it('ignores a precompressed copy older than its file', async () => {
    const file = join(root, 'assets', 'app-Abcdef12.js');
    const future = new Date(statSync(file).mtimeMs + 60_000);
    utimesSync(file, future, future);
    const r = await get('/assets/app-Abcdef12.js', { 'accept-encoding': 'br' });
    expect(r.headers.get('content-encoding')).toBeNull();
    expect(resolve(file)).toBe(file);
  });
});
