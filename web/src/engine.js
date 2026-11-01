// The Neon module in Web Workers: one Engine is one worker with one instance, asked with JSON requests.

let compiled = null;

/** Compiles the module once; every worker is then instantiated from the compiled form. */
export function loadModule(url = 'neon.wasm') {
  compiled ??= (async () => {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`не удалось загрузить ${url}: ${response.status}`);
    const type = response.headers.get('content-type') ?? '';
    if (WebAssembly.compileStreaming && type.includes('application/wasm')) {
      return WebAssembly.compileStreaming(response);
    }
    return WebAssembly.compile(await response.arrayBuffer());
  })();
  return compiled;
}

export class Engine {
  constructor(name) {
    this.name = name;
    this.worker = null;
    this.seq = 0;
    this.pending = new Map();
    this.starting = null;
    /** Called after the instance crashed and was replaced. */
    this.onCrash = null;
  }

  start() {
    this.starting ??= (async () => {
      const module = await loadModule();
      const worker = new Worker('worker.js', { name: `neon-${this.name}` });
      this.worker = worker;
      await new Promise((resolve, reject) => {
        worker.onmessage = (e) => (e.data.type === 'ready' ? resolve() : null);
        worker.onerror = (e) => reject(new Error(e.message));
        worker.postMessage({ type: 'init', module });
      });
      worker.onmessage = (e) => this.#message(e.data);
      worker.onerror = (e) => this.#fail(e.message || 'ошибка воркера');
    })();
    return this.starting;
  }

  #message(m) {
    const p = this.pending.get(m.id);
    if (!p) return;
    this.pending.delete(m.id);
    if (m.error !== undefined) {
      const err = new Error(m.error);
      err.crashed = !!m.crashed;
      p.reject(err);
      if (m.crashed && this.onCrash) this.onCrash();
    } else {
      p.resolve(m.result);
    }
  }

  #fail(message) {
    for (const p of this.pending.values()) p.reject(new Error(message));
    this.pending.clear();
  }

  /** Sends a request, resolves with the reply. */
  async call(req) {
    await this.start();
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, req });
    });
  }

  /** Kills the worker (a program that does not stop); the next call starts a new one. */
  terminate() {
    this.worker?.terminate();
    this.worker = null;
    this.starting = null;
    const err = new Error('terminated');
    err.terminated = true;
    for (const p of this.pending.values()) p.reject(err);
    this.pending.clear();
  }
}
