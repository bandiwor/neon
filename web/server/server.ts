import { createReadStream } from 'node:fs';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { pipeline } from 'node:stream/promises';
import type { ServerConfig } from './config';
import { buildCsp } from './csp';
import { resolveRequest } from './files';
import { cacheControl, etag, notModified, securityHeaders } from './headers';
import { contentType } from './mime';

export interface RunningServer {
  readonly url: string;
  readonly server: Server;
  close(): Promise<void>;
}

function plain(res: ServerResponse, status: number, text: string, extra: Record<string, string> = {}): void {
  res.writeHead(status, { 'content-type': 'text/plain; charset=utf-8', ...extra });
  res.end(text);
}

/** Starts the static file server; resolves when it listens. */
export async function startServer(config: ServerConfig, log: (line: string) => void = console.log): Promise<RunningServer> {
  const csp = await buildCsp(config.root);
  const security = securityHeaders(csp);

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const started = performance.now();
    const method = req.method ?? 'GET';
    const url = new URL(req.url ?? '/', 'http://localhost');
    let status = 200;
    try {
      if (method !== 'GET' && method !== 'HEAD') {
        status = 405;
        return plain(res, status, 'method not allowed', { allow: 'GET, HEAD' });
      }
      const found = await resolveRequest(config.root, url.pathname, req.headers['accept-encoding']);
      if (!found) {
        status = 404;
        return plain(res, status, 'not found');
      }
      const tag = etag(found.stats, found.encoding);
      const headers = {
        ...security,
        'content-type': contentType(found.file),
        'cache-control': cacheControl(found.file),
        etag: tag,
        vary: 'Accept-Encoding',
        ...(found.encoding ? { 'content-encoding': found.encoding } : {}),
      };
      if (notModified(req.headers['if-none-match'], tag)) {
        status = 304;
        res.writeHead(status, headers);
        res.end();
        return;
      }
      res.writeHead(status, { ...headers, 'content-length': found.stats.size });
      if (method === 'HEAD') {
        res.end();
        return;
      }
      await pipeline(createReadStream(found.sendFile), res);
    } catch (e) {
      if (res.headersSent) {
        res.destroy();
      } else {
        status = 500;
        plain(res, status, 'internal error');
      }
      log(`error ${url.pathname}: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      if (config.log) log(`${method} ${url.pathname} ${status} ${Math.round(performance.now() - started)}ms`);
    }
  }

  const server = createServer((req, res) => void handle(req, res));
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(config.port, config.host, () => {
      server.off('error', reject);
      resolve();
    });
  });
  const { port } = server.address() as AddressInfo;
  return {
    url: `http://${config.host}:${port}/`,
    server,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
        server.closeAllConnections();
      }),
  };
}
