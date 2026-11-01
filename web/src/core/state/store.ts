// A tiny observable store with optional persistence. No framework in here: components read stores through
// `useStore` (see ./useStore.ts).

export interface Store<T extends object> {
  get(): T;
  /** Merges a patch (or the result of a function of the current state) into the state. */
  set(patch: Partial<T> | ((state: T) => Partial<T>)): void;
  subscribe(listener: () => void): () => void;
}

export interface StoreOptions<T extends object> {
  /** localStorage key: the state is saved (a moment after a change) and restored. */
  readonly persistKey?: string;
  /** Checks what was read back; anything it rejects is replaced by the defaults. */
  readonly revive?: (stored: unknown, defaults: T) => T;
}

function readStored<T extends object>(key: string, defaults: T, revive?: (stored: unknown, defaults: T) => T): T {
  try {
    const raw = localStorage.getItem(key);
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      if (revive) return revive(parsed, defaults);
      if (typeof parsed === 'object' && parsed !== null) return { ...defaults, ...(parsed as Partial<T>) };
    }
  } catch {
    /* damaged or unavailable: start over */
  }
  return defaults;
}

export function createStore<T extends object>(defaults: T, options: StoreOptions<T> = {}): Store<T> {
  const { persistKey, revive } = options;
  let state: T = persistKey ? readStored(persistKey, defaults, revive) : defaults;
  const listeners = new Set<() => void>();
  let timer: ReturnType<typeof setTimeout> | undefined;

  const persist = (): void => {
    if (!persistKey) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      try {
        localStorage.setItem(persistKey, JSON.stringify(state));
      } catch {
        /* private mode, quota: the session just does not survive */
      }
    }, 300);
  };

  return {
    get: () => state,
    set(patch) {
      const next = typeof patch === 'function' ? patch(state) : patch;
      state = { ...state, ...next };
      persist();
      listeners.forEach((l) => l());
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
