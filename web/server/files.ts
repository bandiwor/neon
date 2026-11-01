import type { Stats } from 'node:fs';
import { stat } from 'node:fs/promises';
import { join, sep } from 'node:path';

export type Encoding = 'br' | 'gzip';

export interface Resolved {
  /** The file the URL names (its type and its identity). */
  readonly file: string;
  /** The file whose bytes are sent: `file` itself or its precompressed copy. */
  readonly sendFile: string;
  readonly encoding: Encoding | null;
  readonly stats: Stats;
}

/** The file a URL path points to, inside `root` and nowhere else; null if there is none. */
export async function locate(root: string, urlPath: string): Promise<string | null> {
  let decoded: string;
  try {
    decoded = decodeURIComponent(urlPath);
  } catch {
    return null;
  }
  if (decoded.includes('\0')) return null;
  const file = join(root, decoded);
  if (file !== root && !file.startsWith(root + sep)) return null; // no way out of root
  try {
    const s = await stat(file);
    if (s.isFile()) return file;
    if (s.isDirectory()) {
      const index = join(file, 'index.html');
      return (await stat(index)).isFile() ? index : null;
    }
  } catch {
    /* missing */
  }
  return null;
}

/** Which encodings a request accepts, best first. */
export function acceptedEncodings(header: string | undefined): readonly Encoding[] {
  if (!header) return [];
  const accepted = new Set(
    header
      .split(',')
      .map((part) => part.trim().split(';'))
      .filter(([, q]) => q?.trim() !== 'q=0')
      .map(([name]) => name?.trim().toLowerCase()),
  );
  return (['br', 'gzip'] as const).filter((e) => accepted.has(e));
}

const SUFFIX: Readonly<Record<Encoding, string>> = { br: '.br', gzip: '.gz' };

export async function resolveRequest(root: string, urlPath: string, acceptEncoding: string | undefined): Promise<Resolved | null> {
  const file = await locate(root, urlPath);
  if (!file) return null;
  const stats = await stat(file);
  for (const encoding of acceptedEncodings(acceptEncoding)) {
    const sendFile = file + SUFFIX[encoding];
    try {
      const copy = await stat(sendFile);
      if (copy.isFile() && copy.mtimeMs >= stats.mtimeMs) return { file, sendFile, encoding, stats: copy };
    } catch {
      /* no precompressed copy of that kind */
    }
  }
  return { file, sendFile: file, encoding: null, stats };
}
