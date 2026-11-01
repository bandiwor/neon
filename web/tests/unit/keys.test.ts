import { describe, expect, it } from 'vitest';
import { terminalKey, type KeyLike } from '../../src/core/util/keys';

const key = (k: string, mods: Partial<KeyLike> = {}) =>
  terminalKey({ key: k, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods });

describe('terminalKey', () => {
  it('sends characters as they are', () => {
    expect(key('a')).toEqual({ bytes: 'a', name: 'a' });
    expect(key('A', { shiftKey: true })).toEqual({ bytes: 'A', name: 'A' });
    expect(key(' ')).toEqual({ bytes: ' ', name: ' ' });
    expect(key('я')).toEqual({ bytes: 'я', name: 'я' });
  });

  it('turns controls into control characters', () => {
    expect(key('c', { ctrlKey: true })).toEqual({ bytes: '\u0003', name: 'ctrl+c' });
    expect(key(' ', { ctrlKey: true })?.name).toBe('ctrl+space');
    expect(key('x', { altKey: true })).toEqual({ bytes: '\u001bx', name: 'alt+x' });
    expect(key('Enter')).toEqual({ bytes: '\r', name: 'enter' });
    expect(key('Backspace')?.bytes).toBe('\u007f');
    expect(key('Escape')).toEqual({ bytes: '\u001b', name: 'esc' });
    expect(key('Tab', { shiftKey: true })).toEqual({ bytes: '\u001b[Z', name: 'shift+tab' });
  });

  it('writes the escape sequences of an ANSI terminal', () => {
    expect(key('ArrowUp')).toEqual({ bytes: '\u001b[A', name: 'up' });
    expect(key('ArrowRight', { ctrlKey: true })).toEqual({ bytes: '\u001b[1;5C', name: 'ctrl+right' });
    expect(key('ArrowLeft', { shiftKey: true, altKey: true })?.bytes).toBe('\u001b[1;4D');
    expect(key('PageDown')).toEqual({ bytes: '\u001b[6~', name: 'pagedown' });
    expect(key('Delete', { ctrlKey: true })?.bytes).toBe('\u001b[3;5~');
    expect(key('F1')).toEqual({ bytes: '\u001bOP', name: 'f1' });
    expect(key('F5')?.bytes).toBe('\u001b[15~');
    expect(key('F12')?.bytes).toBe('\u001b[24~');
  });

  it('ignores what means nothing to a program', () => {
    for (const k of ['Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'Dead']) expect(key(k), k).toBeNull();
    expect(key('c', { metaKey: true })).toBeNull();
    expect(key('MediaPlayPause')).toBeNull();
  });
});
