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
    expect(parseAnsi('a\rb\x1b[5nc\r\nd').runs.map((r) => r.text).join('')).toBe('abc\r\nd');
  });

  it('keeps the end of a very long output', () => {
    const out = parseAnsi('x'.repeat(50) + 'END', 10);
    expect(out.truncated).toBe(true);
    expect(out.runs.map((r) => r.text).join('')).toBe('xxxxxxxEND');
  });
});

describe('output that moves the cursor', () => {
  const text = (s: string) => parseAnsi(s).runs.map((r) => r.text).join('');

  it('redraws a frame in place', () => {
    const frame = (n: number) => `\x1b[1;1H+--+\x1b[K\n|${n}${n}|\n+--+\x1b[J`;
    expect(text(`\x1b[?1049h${frame(1)}${frame(2)}`)).toBe('+--+\n|22|\n+--+');
  });

  it('erases to the end of the line and of the screen', () => {
    expect(text('hello world\x1b[1;6H\x1b[K!')).toBe('hello!');
    expect(text('one\ntwo\nthree\x1b[2;1H\x1b[J')).toBe('one');
    expect(text('abc\x1b[2J\x1b[H\x1b[1;1Hxy')).toBe('xy');
  });

  it('moves the cursor around and keeps colors on the grid', () => {
    const { runs } = parseAnsi('ab\x1b[2D\x1b[31mX\x1b[0mY\n\x1b[3Cz\x1b[1;1H\x1b[K');
    expect(runs.map((r) => r.text).join('')).toBe('\n   z');
    const colored = parseAnsi('\x1b[2;3H\x1b[32mq\x1b[0m').runs;
    expect(colored.map((r) => [r.text, r.fg])).toEqual([['\n  ', undefined], ['q', { name: 'green' }]]);
  });

  it('does not touch plain output', () => {
    expect(text('one\ntwo')).toBe('one\ntwo');
    expect(text('old\x1b[2J\x1b[Hnew')).toBe('new');
  });
});
