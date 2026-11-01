import type { Position, Range } from './types';

/** The offset of `pos` in `text` (clamped to its line). */
export function offsetIn(text: string, pos: Position): number {
  let offset = 0;
  for (let line = 0; line < pos.line; line++) {
    const next = text.indexOf('\n', offset);
    if (next < 0) return text.length;
    offset = next + 1;
  }
  const end = text.indexOf('\n', offset);
  return Math.min(offset + pos.character, end < 0 ? text.length : end);
}

export function rangeIn(text: string, range: Range): { from: number; to: number } {
  return { from: offsetIn(text, range.start), to: offsetIn(text, range.end) };
}

export function fileOfUri(uri: string): string {
  return decodeURIComponent(uri.replace(/^file:\/\/\//, ''));
}

export function uriOfFile(name: string): string {
  return `file:///${name}`;
}
