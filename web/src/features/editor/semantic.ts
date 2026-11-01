// The language server's semantic colors laid over the lexical ones.
import { StateEffect, StateField, type Text } from '@codemirror/state';
import { Decoration, EditorView, type DecorationSet } from '@codemirror/view';

export interface SemanticClasses {
  readonly type: string;
  readonly param: string;
  readonly variable: string;
  readonly property: string;
  readonly variant: string;
  readonly function: string;
}

/** Which class a token type (a name from the server's legend) gets. */
function classFor(type: string | undefined, c: SemanticClasses): string | undefined {
  switch (type) {
    case 'namespace':
    case 'type':
    case 'struct':
    case 'enum':
    case 'interface':
    case 'typeParameter':
      return c.type;
    case 'parameter':
      return c.param;
    case 'variable':
      return c.variable;
    case 'property':
      return c.property;
    case 'enumMember':
      return c.variant;
    case 'function':
    case 'method':
      return c.function;
    default:
      return undefined;
  }
}

/** Decorations for the server's encoded semantic tokens (positions are line / UTF-16 column). */
export function semanticDecorations(doc: Text, data: readonly number[], legend: readonly string[], classes: SemanticClasses): DecorationSet {
  const ranges = [];
  let line = 0;
  let col = 0;
  for (let i = 0; i + 4 < data.length; i += 5) {
    const dLine = data[i] ?? 0;
    line += dLine;
    col = dLine === 0 ? col + (data[i + 1] ?? 0) : (data[i + 1] ?? 0);
    const cls = classFor(legend[data[i + 3] ?? -1], classes);
    if (!cls || line + 1 > doc.lines) continue;
    const from = doc.line(line + 1).from + col;
    const to = from + (data[i + 2] ?? 0);
    if (to <= doc.length) ranges.push(Decoration.mark({ class: cls }).range(from, to));
  }
  return Decoration.set(ranges, true);
}

export const setSemantic = StateEffect.define<DecorationSet>();

export const semanticField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, tr) {
    let next = value.map(tr.changes);
    for (const e of tr.effects) if (e.is(setSemantic)) next = e.value;
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});
