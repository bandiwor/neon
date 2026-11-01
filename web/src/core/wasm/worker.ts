// A Web Worker that hosts one instance of the Neon module and answers JSON requests.
import type { ModuleError, WorkerIn, WorkerOut } from './protocol';

interface NeonExports {
  readonly memory: WebAssembly.Memory;
  neon_alloc(len: number): number;
  neon_dealloc(ptr: number, len: number): void;
  neon_call(ptr: number, len: number): number;
}

/** The little of a worker's global scope this file uses (the DOM typings are in force, not WebWorker's). */
interface WorkerScope {
  onmessage: ((e: MessageEvent<WorkerIn>) => void | Promise<void>) | null;
  postMessage(message: WorkerOut): void;
}
const scope = self as unknown as WorkerScope;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

let wasm: WebAssembly.Module | null = null;
let exports: NeonExports | null = null;

async function instantiate(): Promise<NeonExports> {
  if (!wasm) throw new Error('the module is not loaded');
  const instance = await WebAssembly.instantiate(wasm, {});
  exports = instance.exports as unknown as NeonExports;
  return exports;
}

function call(ex: NeonExports, op: string, payload: object): unknown {
  const bytes = encoder.encode(JSON.stringify({ op, ...payload }));
  const ptr = ex.neon_alloc(bytes.length);
  new Uint8Array(ex.memory.buffer, ptr, bytes.length).set(bytes);
  const out = ex.neon_call(ptr, bytes.length);
  ex.neon_dealloc(ptr, bytes.length);
  // the memory may have grown during the call: look at the buffer afterwards
  const len = new DataView(ex.memory.buffer).getUint32(out, true);
  const text = decoder.decode(new Uint8Array(ex.memory.buffer, out + 4, len));
  ex.neon_dealloc(out, len + 4);
  const reply = JSON.parse(text) as unknown;
  if (typeof reply === 'object' && reply !== null && 'error' in reply) throw new Error((reply as ModuleError).error);
  return reply;
}

scope.onmessage = async (e) => {
  const m = e.data;
  if (m.type === 'init') {
    wasm = m.module;
    await instantiate();
    scope.postMessage({ type: 'ready' });
    return;
  }
  try {
    const ex = exports ?? (await instantiate());
    scope.postMessage({ type: 'result', id: m.id, result: call(ex, m.op, m.payload) });
  } catch (err) {
    // a trap inside the module (a bug, or running out of memory): the instance is gone; the next request
    // starts a fresh one, and the host is told so it can restore what the instance held
    const trapped = err instanceof WebAssembly.RuntimeError;
    if (trapped) exports = null;
    scope.postMessage({ type: 'error', id: m.id, message: err instanceof Error ? err.message : String(err), crashed: trapped });
  }
};
