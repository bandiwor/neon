import type { Completion, CompletionContext, CompletionResult } from '@codemirror/autocomplete';
import type { EditorView } from '@codemirror/view';
import type { Hover } from '@/core/lsp/types';
import { offsetToPosition, positionToOffset } from './positions';
import type { EditorHooks } from './createEditor';

const KIND = {
  2: 'method', 3: 'function', 5: 'property', 6: 'variable', 7: 'class', 8: 'interface', 9: 'namespace',
  13: 'enum', 14: 'keyword', 20: 'constant', 21: 'constant', 22: 'class',
} as const satisfies Record<number, string>;

const WORD = /[A-Za-z_\u0080-\uffff][A-Za-z0-9_\u0080-\uffff]*/;

/** What a function's detail says about its arguments: `name(a, b) -> T` has some, `name() -> T` has none. */
function takesArguments(label: string, detail: string): boolean {
  if (!detail.startsWith(`${label}(`)) return true;
  const inside = /^[^(]*\(([^)]*)\)/.exec(detail)?.[1] ?? '';
  return inside.replace(/^\s*self\s*(,|$)/, '').trim() !== '';
}

/** Completing a function writes the call too: `name(` with the caret inside, or `name()` with it after. */
function callApply(label: string, detail: string): (view: EditorView, completion: Completion, from: number, to: number) => void {
  return (view: EditorView, _completion: Completion, from: number, to: number) => {
    const open = view.state.sliceDoc(to, to + 1) === '(';
    const inside = takesArguments(label, detail);
    const insert = open ? label : `${label}()`;
    const caret = from + label.length + (open ? 0 : inside ? 1 : 2);
    view.dispatch({ changes: { from, to, insert }, selection: { anchor: open ? from + label.length : caret }, userEvent: 'input.complete' });
  };
}

export function completionSource(hooks: EditorHooks, file: () => string | null) {
  return async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const name = file();
    if (!name) return null;
    const word = ctx.matchBefore(WORD);
    const before = ctx.state.sliceDoc(Math.max(0, ctx.pos - 2), ctx.pos);
    if (!word && !ctx.explicit && !before.endsWith('.') && !before.endsWith('::')) return null;
    await hooks.flush();
    let result: Awaited<ReturnType<EditorHooks['lsp']['completion']>>;
    try {
      result = await hooks.lsp.completion(name, offsetToPosition(ctx.state.doc, ctx.pos));
    } catch {
      return null;
    }
    if (ctx.aborted || !result) return null;
    const items = 'items' in result ? result.items : result;
    return {
      from: word ? word.from : ctx.pos,
      options: items.map((it) => {
        const detail = it.detail ?? '';
        const kind = it.kind === undefined ? undefined : (KIND as Readonly<Record<number, string>>)[it.kind];
        return {
          label: it.label,
          ...(kind === 'function' || kind === 'method' ? { apply: callApply(it.label, detail) } : {}),
          ...(detail && detail.length < 36 ? { detail } : {}),
          ...(detail && detail.length >= 36 ? { info: detail } : {}),
          type: kind ?? 'text',
          boost: it.sortText ? -(it.sortText.charCodeAt(0) - 48) : 0,
        };
      }),
      validFor: /^[A-Za-z0-9_\u0080-\uffff]*$/,
    };
  };
}

/** Hover contents as DOM: fenced blocks are code, the rest is prose. */
export function renderHover(h: Hover, className: string): { dom: HTMLElement; from: (doc: import('@codemirror/state').Text, pos: number) => { from: number; to: number } } | null {
  const value = typeof h.contents === 'string' ? h.contents : h.contents.value;
  if (!value) return null;
  const dom = document.createElement('div');
  dom.className = className;
  value.split(/```[a-z]*\n?/).forEach((part, i) => {
    const text = part.replace(/\n+$/, '').trim();
    if (!text) return;
    const el = document.createElement(i % 2 === 1 ? 'pre' : 'p');
    el.textContent = text;
    dom.append(el);
  });
  const range = h.range;
  return {
    dom,
    from: (doc, pos) =>
      range ? { from: positionToOffset(doc, range.start), to: positionToOffset(doc, range.end) } : { from: pos, to: pos },
  };
}
