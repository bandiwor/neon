import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { Command } from '@/app/commands';
import { t } from '@/core/i18n';
import { runtime } from '@/core/state/stores';
import { Icon, cx } from '@/ui';
import s from './Palette.module.scss';

/** The command palette: type to filter, arrows to choose, Enter to run. */
export function Palette({ commands }: { readonly commands: readonly Command[] }): JSX.Element {
  const [query, setQuery] = useState('');
  const [index, setIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const close = (): void => runtime.set({ dialog: null });

  useEffect(() => {
    const before = document.activeElement;
    input.current?.focus();
    const escape = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('keydown', escape);
    return () => {
      window.removeEventListener('keydown', escape);
      if (before instanceof HTMLElement) before.focus();
    };
  }, []);

  const list = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return commands.filter((c) => !needle || c.title.toLowerCase().includes(needle));
  }, [query, commands]);
  useEffect(() => setIndex(0), [query]);

  const go = (c: Command): void => {
    close();
    setTimeout(c.run, 0);
  };
  const last = Math.max(1, list.length);

  return (
    <div class={s.backdrop} onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div class={s.palette} role="dialog" aria-modal="true" aria-label={t('palette.title')}>
        <div class={s.search}>
          <Icon name="search" />
          <input
            ref={input}
            class={s.input}
            value={query}
            placeholder={t('palette.placeholder')}
            onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown') {
                e.preventDefault();
                setIndex((index + 1) % last);
              } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setIndex((index + last - 1) % last);
              } else if (e.key === 'Enter') {
                const chosen = list[index];
                if (chosen) {
                  e.preventDefault();
                  go(chosen);
                }
              }
            }}
            spellcheck={false}
          />
        </div>
        <ul class={s.list} role="listbox">
          {list.map((c, k) => (
            <li key={c.id} role="option" aria-selected={k === index} class={cx(k === index && s.current)} onPointerMove={() => setIndex(k)} onClick={() => go(c)}>
              <span>{c.title}</span>
              {c.hint && <kbd>{c.hint}</kbd>}
            </li>
          ))}
          {list.length === 0 && <li class={s.none}>{t('palette.none')}</li>}
        </ul>
      </div>
    </div>
  );
}
