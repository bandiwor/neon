// Sharing: the project is packed into the address (`#p=...`), so a link is the whole program.
import type { SourceFile } from '../project/types';

async function pipe(bytes: Uint8Array<ArrayBuffer>, stream: CompressionStream | DecompressionStream): Promise<Uint8Array> {
  const out = new Response(new Blob([bytes]).stream().pipeThrough(stream));
  return new Uint8Array(await out.arrayBuffer());
}

function toBase64Url(bytes: Uint8Array): string {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

function fromBase64Url(text: string): Uint8Array<ArrayBuffer> {
  const s = atob(text.replaceAll('-', '+').replaceAll('_', '/'));
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

export interface SharedProject {
  readonly files: readonly SourceFile[];
  readonly entry: string | undefined;
}

/** The address fragment for a project. */
export async function encodeProject(files: readonly SourceFile[], entry: string): Promise<string> {
  const json = new TextEncoder().encode(JSON.stringify({ f: files.map((f) => [f.name, f.text]), e: entry }));
  const packed = await pipe(json, new CompressionStream('deflate-raw'));
  return `p=${toBase64Url(packed)}`;
}

/** The project in an address fragment, or null. */
export async function decodeProject(hash: string): Promise<SharedProject | null> {
  const m = /^#?p=([A-Za-z0-9_-]+)$/.exec(hash);
  if (!m?.[1]) return null;
  try {
    const raw = await pipe(fromBase64Url(m[1]), new DecompressionStream('deflate-raw'));
    const data: unknown = JSON.parse(new TextDecoder().decode(raw));
    if (typeof data !== 'object' || data === null) return null;
    const { f, e } = data as { f?: unknown; e?: unknown };
    if (!Array.isArray(f) || f.length === 0) return null;
    const files: SourceFile[] = [];
    for (const item of f as unknown[]) {
      if (!Array.isArray(item)) return null;
      files.push({ name: String(item[0]), text: String(item[1]) });
    }
    return { files, entry: typeof e === 'string' ? e : undefined };
  } catch {
    return null;
  }
}
