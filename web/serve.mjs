// A small asynchronous static file server for dist/ (everything else happens in the browser).
//
//   node serve.mjs [port]      http://localhost:8080 by default; PORT works too
import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { dirname, extname, join, normalize, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), 'dist');
const port = Number(process.argv[2] ?? process.env.PORT ?? 8080);

const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.svg': 'image/svg+xml',
  '.neon': 'text/plain; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
};

async function resolveFile(urlPath) {
  const clean = normalize(decodeURIComponent(urlPath)).replace(/^([/\\])+/, '');
  let file = join(root, clean);
  if (file !== root && !file.startsWith(root + sep)) return null; // no way out of dist/
  try {
    const s = await stat(file);
    if (s.isDirectory()) file = join(file, 'index.html');
    return { file, size: (await stat(file)).size };
  } catch {
    return null;
  }
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  const found = await resolveFile(url.pathname);
  if (!found || (req.method !== 'GET' && req.method !== 'HEAD')) {
    res.writeHead(found ? 405 : 404, { 'content-type': 'text/plain; charset=utf-8' });
    res.end(found ? 'method not allowed' : 'not found');
    return;
  }
  res.writeHead(200, {
    'content-type': types[extname(found.file)] ?? 'application/octet-stream',
    'content-length': found.size,
    'cache-control': 'no-cache',
  });
  if (req.method === 'HEAD') return void res.end();
  createReadStream(found.file).on('error', () => res.destroy()).pipe(res);
});

server.listen(port, () => console.log(`Neon: http://localhost:${port}  (serving ${root})`));
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => server.close(() => process.exit(0)));
