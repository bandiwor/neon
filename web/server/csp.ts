import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

/** The `sha256-...` source of an inline script. */
export function scriptHash(source: string): string {
  return `'sha256-${createHash('sha256').update(source).digest('base64')}'`;
}

/** The bodies of the inline `<script>` elements (those without `src`) of an HTML document. */
export function inlineScripts(html: string): string[] {
  const out: string[] = [];
  for (const m of html.matchAll(/<script(?![^>]*\ssrc=)[^>]*>([\s\S]*?)<\/script>/g)) if (m[1]) out.push(m[1]);
  return out;
}

/**
 * The Content-Security-Policy for the site in `root`: nothing but its own files, plus the inline scripts of
 * index.html (by hash), the WebAssembly the module compiles, workers, and inline styles (the editor writes
 * its own style elements). Null when there is no index.html.
 */
export async function buildCsp(root: string): Promise<string | null> {
  let html: string;
  try {
    html = await readFile(join(root, 'index.html'), 'utf8');
  } catch {
    return null;
  }
  const hashes = inlineScripts(html).map(scriptHash);
  return [
    "default-src 'self'",
    `script-src 'self' ${hashes.join(' ')} 'wasm-unsafe-eval'`.replace(/ +/g, ' '),
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data:",
    "worker-src 'self' blob:",
    "connect-src 'self'",
    "object-src 'none'",
    "base-uri 'self'",
    "frame-ancestors 'none'",
  ].join('; ');
}
