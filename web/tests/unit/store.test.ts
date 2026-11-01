import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createStore } from '../../src/core/state/store';

class MemoryStorage {
  readonly data = new Map<string, string>();
  getItem(k: string): string | null {
    return this.data.get(k) ?? null;
  }
  setItem(k: string, v: string): void {
    this.data.set(k, v);
  }
}

describe('createStore', () => {
  let storage: MemoryStorage;
  beforeEach(() => {
    storage = new MemoryStorage();
    vi.stubGlobal('localStorage', storage);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('merges patches and tells the subscribers', () => {
    const store = createStore({ a: 1, b: 'x' });
    const seen: number[] = [];
    const off = store.subscribe(() => seen.push(store.get().a));
    store.set({ a: 2 });
    store.set((s) => ({ a: s.a + 1 }));
    off();
    store.set({ a: 9 });
    expect(seen).toEqual([2, 3]);
    expect(store.get()).toEqual({ a: 9, b: 'x' });
  });

  it('saves a moment after a change and restores on the next start', () => {
    const first = createStore({ n: 0 }, { persistKey: 'k' });
    first.set({ n: 5 });
    first.set({ n: 6 });
    expect(storage.data.size).toBe(0);
    vi.advanceTimersByTime(400);
    expect(JSON.parse(storage.data.get('k') ?? '{}')).toEqual({ n: 6 });
    expect(createStore({ n: 0 }, { persistKey: 'k' }).get()).toEqual({ n: 6 });
  });

  it('survives damaged storage and uses the reviver', () => {
    storage.setItem('k', '{not json');
    expect(createStore({ n: 1 }, { persistKey: 'k' }).get()).toEqual({ n: 1 });
    storage.setItem('k', JSON.stringify({ n: 'text' }));
    const strict = createStore({ n: 1 }, { persistKey: 'k', revive: (s, d) => (typeof (s as { n: unknown }).n === 'number' ? (s as typeof d) : d) });
    expect(strict.get()).toEqual({ n: 1 });
  });
});
