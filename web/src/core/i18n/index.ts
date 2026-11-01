// Interface language. Only Russian for now; adding a language is adding a file to `locales` and a line there.
import { LOCALES, ru, type MessageKey } from '@/locales';
import { settings } from '../state/stores';

export type { MessageKey };

/** The text for `key` with `{name}` placeholders filled from `params`. */
export function t(key: MessageKey, params?: Readonly<Record<string, string | number>>): string {
  const text = (LOCALES[settings.get().lang] ?? ru)[key];
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => (name in params ? String(params[name]) : whole));
}

/** Russian plural: `plural(5, ['файл', 'файла', 'файлов'])` -> 'файлов'. */
export function plural(n: number, forms: readonly [string, string, string]): string {
  const a = Math.abs(n) % 100;
  const b = a % 10;
  if (a > 10 && a < 20) return forms[2];
  if (b > 1 && b < 5) return forms[1];
  if (b === 1) return forms[0];
  return forms[2];
}

/** `3 файла` */
export function count(n: number, forms: readonly [string, string, string]): string {
  return `${n} ${plural(n, forms)}`;
}
