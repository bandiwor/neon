import { runtime } from '../state/stores';
import type { ToastKind } from '../state/types';

/** Shows a message at the bottom of the page. */
export function toast(text: string, kind: ToastKind = 'info', action: { label: string; run: () => void } | null = null): void {
  runtime.set({ toast: { text, kind, action, nonce: Math.random() } });
}
