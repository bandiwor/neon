// A Web Worker that hosts one instance of the Neon module and answers JSON requests.
// Protocol: {type: 'init', module} -> {type: 'ready'}; {id, req} -> {id, result} | {id, error}.
let exports = null;
let moduleToLoad = null;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

async function instantiate() {
  const { exports: ex } = await WebAssembly.instantiate(moduleToLoad, {});
  exports = ex;
}

function call(req) {
  const bytes = encoder.encode(JSON.stringify(req));
  const ptr = exports.neon_alloc(bytes.length);
  new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
  const out = exports.neon_call(ptr, bytes.length);
  exports.neon_dealloc(ptr, bytes.length);
  // the memory may have grown during the call: look at the buffer afterwards
  const len = new DataView(exports.memory.buffer).getUint32(out, true);
  const text = decoder.decode(new Uint8Array(exports.memory.buffer, out + 4, len));
  exports.neon_dealloc(out, len + 4);
  return JSON.parse(text);
}

self.onmessage = async (e) => {
  const m = e.data;
  if (m.type === 'init') {
    moduleToLoad = m.module;
    await instantiate();
    self.postMessage({ type: 'ready' });
    return;
  }
  const { id, req } = m;
  try {
    if (!exports) await instantiate();
    self.postMessage({ id, result: call(req) });
  } catch (err) {
    // a trap inside the module (a bug, or running out of memory): the instance is gone; the next request
    // starts a fresh one, and the host is told so it can restore what the instance held
    exports = null;
    self.postMessage({ id, error: String(err && err.message ? err.message : err), crashed: true });
  }
};
