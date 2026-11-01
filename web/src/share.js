// Sharing: the project is packed into the address (`#p=...`), so a link is the whole program.

async function pipe(bytes, stream) {
  const out = new Response(new Blob([bytes]).stream().pipeThrough(stream));
  return new Uint8Array(await out.arrayBuffer());
}

function toBase64Url(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

function fromBase64Url(text) {
  const s = atob(text.replaceAll('-', '+').replaceAll('_', '/'));
  return Uint8Array.from(s, (c) => c.charCodeAt(0));
}

/** The address fragment for a project: {files: [{name, text}], entry}. */
export async function encodeProject(files, entry) {
  const json = new TextEncoder().encode(JSON.stringify({ f: files.map((f) => [f.name, f.text]), e: entry }));
  const packed = await pipe(json, new CompressionStream('deflate-raw'));
  return `p=${toBase64Url(packed)}`;
}

/** The project in an address fragment, or null. */
export async function decodeProject(hash) {
  const m = /^#?p=([A-Za-z0-9_-]+)$/.exec(hash);
  if (!m) return null;
  try {
    const raw = await pipe(fromBase64Url(m[1]), new DecompressionStream('deflate-raw'));
    const data = JSON.parse(new TextDecoder().decode(raw));
    if (!Array.isArray(data.f) || data.f.length === 0) return null;
    return { files: data.f.map(([name, text]) => ({ name: String(name), text: String(text) })), entry: data.e };
  } catch {
    return null;
  }
}
