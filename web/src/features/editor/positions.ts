import type { Text } from '@codemirror/state';
import type { Position } from '@/core/lsp/types';

export function offsetToPosition(doc: Text, offset: number): Position {
  const line = doc.lineAt(offset);
  return { line: line.number - 1, character: offset - line.from };
}

export function positionToOffset(doc: Text, pos: Position): number {
  if (pos.line + 1 > doc.lines) return doc.length;
  const line = doc.line(pos.line + 1);
  return Math.min(line.from + pos.character, line.to);
}
