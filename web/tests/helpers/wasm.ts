// Loads the Neon module in Node and talks to it the way the worker does.
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Op, ReplyOf, RequestOf } from '../../src/core/wasm/protocol';

export const wasmPath = resolve(import.meta.dirname, '../../generated/neon.wasm');
export const wasmBuilt = existsSync(wasmPath);

interface Exports {
  readonly memory: WebAssembly.Memory;
  neon_alloc(len: number): number;
  neon_dealloc(ptr: number, len: number): void;
  neon_call(ptr: number, len: number): number;
}

export type Call = <K extends Op>(op: K, payload: RequestOf<K>) => ReplyOf<K>;

/** A fresh instance of the module. */
export async function loadModule(): Promise<Call> {
  const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
  const x = instance.exports as unknown as Exports;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  return <K extends Op>(op: K, payload: RequestOf<K>): ReplyOf<K> => {
    const bytes = enc.encode(JSON.stringify({ op, ...payload }));
    const ptr = x.neon_alloc(bytes.length);
    new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
    const out = x.neon_call(ptr, bytes.length);
    x.neon_dealloc(ptr, bytes.length);
    const len = new DataView(x.memory.buffer).getUint32(out, true);
    const text = dec.decode(new Uint8Array(x.memory.buffer, out + 4, len));
    x.neon_dealloc(out, len + 4);
    return JSON.parse(text) as ReplyOf<K>;
  };
}
