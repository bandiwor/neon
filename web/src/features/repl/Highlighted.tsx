import type { JSX } from 'preact';
import type { HighlightRun } from '@/core/wasm/protocol';
import s from './Repl.module.scss';

const CLASS: Readonly<Record<string, string | undefined>> = {
  keyword: s.hlKeyword,
  type: s.hlType,
  variant: s.hlVariant,
  global: s.hlGlobal,
  param: s.hlParam,
  local: s.hlLocal,
  function: s.hlFunction,
  number: s.hlNumber,
  string: s.hlString,
  operator: s.hlOperator,
  punct: s.hlPunct,
  bracket: s.hlPunct,
  comment: s.hlComment,
};

/** `text` with the classes the highlighter found; runs are `[from, to, class]` in characters. */
export function Highlighted({ text, runs }: { readonly text: string; readonly runs: readonly HighlightRun[] }): JSX.Element {
  const chars = [...text];
  const out: (string | JSX.Element)[] = [];
  let at = 0;
  for (const [from, to, cls] of runs) {
    if (from > at) out.push(chars.slice(at, from).join(''));
    out.push(<span class={CLASS[cls]}>{chars.slice(from, to).join('')}</span>);
    at = to;
  }
  if (at < chars.length) out.push(chars.slice(at).join(''));
  return <>{out}</>;
}
