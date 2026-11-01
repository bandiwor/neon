import type { CompletionContext, CompletionResult } from '@codemirror/autocomplete';
import type { Hover } from '@/core/lsp/types';
import { offsetToPosition, positionToOffset } from './positions';
import type { EditorHooks } from './createEditor';

const KIND = {
  2: 'method', 3: 'function', 5: 'property', 6: 'variable', 7: 'class', 8: 'interface', 9: 'namespace',
  13: 'enum', 14: 'keyword', 20: 'constant', 21: 'constant', 22: 'class',
} as const satisfies Record<number, string>;

const WORD = /[A-Za-z_\u0080-\uffff][A-Za-z0-9_\u0080-\uffff]*/;

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
