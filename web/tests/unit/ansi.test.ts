import { describe, expect, it } from 'vitest';
import { parseAnsi } from '../../src/core/util/ansi';

describe('parseAnsi', () => {
  it('keeps plain text as one run', () => {
    expect(parseAnsi('hello\nworld').runs).toEqual([{ text: 'hello\nworld' }]);
  });

  it('applies colors, bold and resets', () => {
    const { runs } = parseAnsi('a\x1b[31mred\x1b[1;32mgreen bold\x1b[0mplain');
    expect(runs.map((r) => [r.text, r.fg, r.bold])).toEqual([
      ['a', undefined, undefined],
      ['red', { name: 'red' }, undefined],
      ['green bold', { name: 'green' }, true],
      ['plain', undefined, undefined],
    ]);
  });

  it('reads 256-color and true-color sequences', () => {
    const { runs } = parseAnsi('\x1b[38;5;196mx\x1b[38;2;1;2;3my');
    expect(runs[0]?.fg).toEqual({ rgb: [255, 0, 0] });
    expect(runs[1]?.fg).toEqual({ rgb: [1, 2, 3] });
  });

  it('drops what came before a clear-screen', () => {
    expect(parseAnsi('old frame\x1b[2J\x1b[Hnew frame').runs.map((r) => r.text).join('')).toBe('new frame');
  });

  it('drops carriage returns that are not line ends and unknown sequences', () => {
    expect(parseAnsi('a\rb\x1b[5Cc\r\nd').runs.map((r) => r.text).join('')).toBe('abc\r\nd');
  });

  it('keeps the end of a very long output', () => {
    const out = parseAnsi('x'.repeat(50) + 'END', 10);
    expect(out.truncated).toBe(true);
    expect(out.runs.map((r) => r.text).join('')).toBe('xxxxxxxEND');
  });
});
