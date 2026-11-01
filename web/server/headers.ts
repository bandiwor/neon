import type { Stats } from 'node:fs';
import { basename } from 'node:path';
import type { OutgoingHttpHeaders } from 'node:http';

/** Hashed files under assets/ never change; everything else is revalidated. */
export function cacheControl(file: string): string {
  return /[\\/]assets[\\/]/.test(file) && /-[A-Za-z0-9_-]{6,}\./.test(basename(file)) ? 'public, max-age=31536000, immutable' : 'no-cache';
}

/** A weak ETag from the size and the modification time of the file (and the encoding it is sent in). */
export function etag(stats: Pick<Stats, 'size' | 'mtimeMs'>, encoding: string | null = null): string {
  return `W/"${stats.size.toString(16)}-${Math.floor(stats.mtimeMs).toString(16)}${encoding ? `-${encoding}` : ''}"`;
}

/** Whether `If-None-Match` names this ETag. */
export function notModified(ifNoneMatch: string | undefined, tag: string): boolean {
  if (!ifNoneMatch) return false;
  return ifNoneMatch.split(',').some((t) => t.trim() === tag || t.trim() === '*');
}

export function securityHeaders(csp: string | null): OutgoingHttpHeaders {
  return {
    'x-content-type-options': 'nosniff',
    'referrer-policy': 'no-referrer',
    'x-frame-options': 'DENY',
    ...(csp ? { 'content-security-policy': csp } : {}),
  };
}
