// The Neon module in Web Workers: one Engine is one worker with one instance, asked with JSON requests.
import type { Op, ReplyOf, RequestOf, WorkerIn, WorkerOut } from './protocol';
import wasmUrl from '../../../generated/neon.wasm?url';

let compiled: Promise<WebAssembly.Module> | null = null;

/** Compiles the module once; every worker is then instantiated from the compiled form. */
export function loadModule(url: string = wasmUrl): Promise<WebAssembly.Module> {
  compiled ??= (async () => {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`не удалось загрузить ${url}: ${response.status}`);
    const type = response.headers.get('content-type') ?? '';
    if (typeof WebAssembly.compileStreaming === 'function' && type.includes('application/wasm')) {
      return WebAssembly.compileStreaming(response);
    }
    return WebAssembly.compile(await response.arrayBuffer());
  })();
  return compiled;
}

/** A request that was cut short because its worker was terminated. */
export class TerminatedError extends Error {
  constructor() {
    super('terminated');
    this.name = 'TerminatedError';
  }
}

/** The module trapped; the instance was replaced by a fresh one. */
export class CrashedError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CrashedError';
  }
}

interface Pending {
  readonly resolve: (value: unknown) => void;
  readonly reject: (reason: Error) => void;
}

export class Engine {
  private worker: Worker | null = null;
  private seq = 0;
  private readonly pending = new Map<number, Pending>();
  private starting: Promise<Worker> | null = null;

  /** Called after the instance crashed and was replaced. */
  onCrash: (() => void) | null = null;

  constructor(readonly name: string) {}

  start(): Promise<Worker> {
    this.starting ??= (async () => {
      const module = await loadModule();
      const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module', name: `neon-${this.name}` });
      this.worker = worker;
      await new Promise<void>((resolve, reject) => {
        worker.onmessage = (e: MessageEvent<WorkerOut>) => {
          if (e.data.type === 'ready') resolve();
        };
        worker.onerror = (e) => reject(new Error(e.message));
        const init: WorkerIn = { type: 'init', module };
        worker.postMessage(init);
      });
      worker.onmessage = (e: MessageEvent<WorkerOut>) => this.dispatch(e.data);
      worker.onerror = (e) => this.failAll(new Error(e.message || 'ошибка воркера'));
      return worker;
    })();
    return this.starting;
  }

  private dispatch(m: WorkerOut): void {
    if (m.type === 'ready') return;
    const p = this.pending.get(m.id);
    if (!p) return;
    this.pending.delete(m.id);
    if (m.type === 'result') {
      p.resolve(m.result);
      return;
    }
    p.reject(m.crashed ? new CrashedError(m.message) : new Error(m.message));
    if (m.crashed) this.onCrash?.();
  }

  private failAll(error: Error): void {
    for (const p of this.pending.values()) p.reject(error);
    this.pending.clear();
  }

  /** Sends a request, resolves with the reply. */
  async call<K extends Op>(op: K, payload: RequestOf<K>): Promise<ReplyOf<K>> {
    const worker = await this.start();
    const id = ++this.seq;
    return new Promise<ReplyOf<K>>((resolve, reject) => {
      this.pending.set(id, { resolve: (v) => resolve(v as ReplyOf<K>), reject });
      const message: WorkerIn = { type: 'call', id, op, payload };
      worker.postMessage(message);
    });
  }

  /** Kills the worker (a program that does not stop); the next call starts a new one. */
  terminate(): void {
    this.worker?.terminate();
    this.worker = null;
    this.starting = null;
    this.failAll(new TerminatedError());
  }
}
