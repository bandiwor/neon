import { useEffect, useRef, useState } from 'preact/hooks';
import type { Store } from './store';

/** Reads (a part of) a store in a component; the component renders again when that part changes. */
export function useStore<T extends object>(store: Store<T>): T;
export function useStore<T extends object, S>(store: Store<T>, select: (state: T) => S): S;
export function useStore<T extends object, S>(store: Store<T>, select?: (state: T) => S): S | T {
  const pick = (select ?? ((s: T) => s)) as (state: T) => S | T;
  const [, setTick] = useState(0);
  const value = pick(store.get());
  const last = useRef<S | T>(value);
  last.current = value;
  const pickRef = useRef(pick);
  pickRef.current = pick;
  useEffect(
    () =>
      store.subscribe(() => {
        const next = pickRef.current(store.get());
        if (!Object.is(next, last.current)) setTick((n) => n + 1);
      }),
    [store],
  );
  return value;
}
