// Interface language. Only Russian for now; adding a language is adding a file to `locales` and a line here.
import { settings } from './store.js';
import ru from './locales/ru.js';

export const LOCALES = { ru };

function dictionary() {
  return LOCALES[settings.get().lang] ?? ru;
}

/** The text for `key` with `{name}` placeholders filled from `params`. */
export function t(key, params) {
  const text = dictionary()[key] ?? ru[key];
  if (text === undefined) {
    if (import.meta.env?.DEV) console.warn(`i18n: no text for ${key}`);
    return key;
  }
  return params ? text.replace(/\{(\w+)\}/g, (_, k) => (k in params ? String(params[k]) : `{${k}}`)) : text;
}

/** Russian plural: `plural(5, ['файл', 'файла', 'файлов'])` -> 'файлов'. */
export function plural(n, forms) {
  const a = Math.abs(n) % 100;
  const b = a % 10;
  if (a > 10 && a < 20) return forms[2];
  if (b > 1 && b < 5) return forms[1];
  if (b === 1) return forms[0];
  return forms[2];
}

/** `3 файла` */
export function count(n, forms) {
  return `${n} ${plural(n, forms)}`;
}
