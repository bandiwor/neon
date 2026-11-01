import ru from './ru';

/** The keys every language must have (the Russian dictionary is the reference). */
export type MessageKey = keyof typeof ru;

/** A dictionary: the same keys, plain strings. */
export type Dictionary = Readonly<Record<MessageKey, string>>;

export type Locale = 'ru';

/** Adding a language: a file next to `ru.ts` with a `Dictionary`, and a line here. */
export const LOCALES: Readonly<Record<Locale, Dictionary>> = { ru };

export const DEFAULT_LOCALE: Locale = 'ru';

export { ru };
