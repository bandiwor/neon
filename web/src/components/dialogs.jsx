import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { Button, Dialog, Field, FocusInput, Segmented, Switch } from './widgets.jsx';
import { Icon } from './icons.jsx';
import { DEFAULT_SETTINGS, runtime, settings, useStore } from '../store.js';
import { t } from '../i18n.js';
import { commitRename } from '../actions.js';

const mac = /Mac|iPhone|iPad/.test(navigator.platform);
export const MOD = mac ? '⌘' : 'Ctrl';

export function Dialogs() {
  const dialog = useStore(runtime, (s) => s.dialog);
  const close = () => runtime.set({ dialog: null });
  if (dialog === 'settings') return <SettingsDialog onClose={close} />;
  if (dialog === 'help') return <HelpDialog onClose={close} />;
  if (dialog === 'palette') return <Palette onClose={close} />;
  if (dialog === 'rename') return <RenameDialog onClose={close} />;
  if (dialog === 'share') return <ShareDialog onClose={close} />;
  return null;
}

// ---- settings --------------------------------------------------------------------------------------------------

function SettingsDialog({ onClose }) {
  const s = useStore(settings);
  const set = (patch) => settings.set(patch);
  const accents = [
    ['ember', t('accent.ember'), '#c2410c'],
    ['moss', t('accent.moss'), '#2f7d4f'],
    ['azure', t('accent.azure'), '#2563b8'],
    ['graphite', t('accent.graphite'), '#52524c'],
  ];
  return (
    <Dialog
      title={t('settings.title')}
      onClose={onClose}
      wide
      footer={
        <>
          <Button icon="reset" onClick={() => confirm(t('settings.confirm_reset')) && set({ ...DEFAULT_SETTINGS })}>
            {t('settings.reset')}
          </Button>
          <span class="spacer" />
          <Button primary onClick={onClose}>
            {t('common.close')}
          </Button>
        </>
      }
    >
      <div class="settings">
        <section>
          <h3>{t('settings.appearance')}</h3>
          <Field label={t('settings.theme')}>
            <Segmented
              value={s.theme}
              onChange={(v) => set({ theme: v })}
              label={t('settings.theme')}
              options={[
                { value: 'system', icon: 'monitor', label: t('theme.system') },
                { value: 'light', icon: 'sun', label: t('theme.light') },
                { value: 'dark', icon: 'moon', label: t('theme.dark') },
              ]}
            />
          </Field>
          <Field label={t('settings.accent')}>
            <div class="swatches" role="radiogroup" aria-label={t('settings.accent')}>
              {accents.map(([id, label, color]) => (
                <button type="button" role="radio" aria-checked={s.accent === id} class={`swatch ${s.accent === id ? 'is-active' : ''}`} style={{ '--swatch': color }} onClick={() => set({ accent: id })} title={label}>
                  <span class="swatch-dot" />
                  <span>{label}</span>
                </button>
              ))}
            </div>
          </Field>
          <Field label={t('settings.font_size')}>
            <div class="range">
              <input type="range" min="11" max="22" step="1" value={s.fontSize} onInput={(e) => set({ fontSize: Number(e.currentTarget.value) })} />
              <span class="muted">{s.fontSize} px</span>
            </div>
          </Field>
          <Switch checked={s.wrap} onChange={(v) => set({ wrap: v })}>
            {t('settings.wrap')}
          </Switch>
          <Field label={t('settings.panel_pos')}>
            <Segmented
              value={s.panelPos}
              onChange={(v) => set({ panelPos: v })}
              label={t('settings.panel_pos')}
              options={[
                { value: 'auto', label: t('panelpos.auto') },
                { value: 'right', icon: 'panel-right', label: t('panelpos.right') },
                { value: 'bottom', icon: 'panel-bottom', label: t('panelpos.bottom') },
              ]}
            />
          </Field>
        </section>
        <section>
          <h3>{t('settings.run')}</h3>
          <Field label={t('settings.opt')} hint={t('settings.opt_hint')}>
            <select value={s.opt} onChange={(e) => set({ opt: e.currentTarget.value })}>
              {['default', 'none', 'no-inline', 'no-peephole'].map((o) => (
                <option value={o}>{t(`opt.${o}`)}</option>
              ))}
            </select>
          </Field>
          <Field label={t('settings.max_steps')} hint={t('settings.max_steps_hint')}>
            <input type="number" min="0" step="1000000" value={s.maxSteps} onInput={(e) => set({ maxSteps: Math.max(0, Number(e.currentTarget.value) || 0) })} />
          </Field>
          <Field label={t('settings.timeout')} hint={t('settings.timeout_hint')}>
            <input type="number" min="0" max="600" value={s.timeout} onInput={(e) => set({ timeout: Math.max(0, Number(e.currentTarget.value) || 0) })} />
          </Field>
          <Field label={t('settings.gc_limit')} hint={t('settings.gc_limit_hint')}>
            <input value={s.gcLimit} placeholder="64m" spellcheck={false} onInput={(e) => set({ gcLimit: e.currentTarget.value })} />
          </Field>
          <Switch checked={s.gcStress} onChange={(v) => set({ gcStress: v })}>
            {t('settings.gc_stress')}
          </Switch>
          <Switch checked={s.stats} onChange={(v) => set({ stats: v })}>
            {t('settings.stats')}
          </Switch>
        </section>
      </div>
    </Dialog>
  );
}

// ---- help ------------------------------------------------------------------------------------------------------

export const SHORTCUTS = [
  ['run', `${MOD}+Enter`],
  ['stop', `${MOD}+.`],
  ['format', `${MOD}+Shift+F`],
  ['check', `${MOD}+Shift+B`],
  ['palette', `${MOD}+K`],
  ['sidebar', `${MOD}+B`],
  ['panel', `${MOD}+J`],
  ['complete', `${MOD}+Space`],
  ['definition', `F12 / ${MOD}+клик`],
  ['rename', 'F2'],
  ['find', `${MOD}+F`],
  ['comment', `${MOD}+/`],
  ['help', 'F1'],
];

function HelpDialog({ onClose }) {
  return (
    <Dialog title={t('help.title')} onClose={onClose} wide>
      <div class="help">
        <p>{t('help.intro')}</p>
        <h3>{t('help.shortcuts')}</h3>
        <dl class="shortcuts">
          {SHORTCUTS.map(([id, keys]) => (
            <>
              <dt>{t(`shortcut.${id}`)}</dt>
              <dd>
                <kbd>{keys}</kbd>
              </dd>
            </>
          ))}
        </dl>
        <h3>{t('help.panels')}</h3>
        <ul>
          <li>{t('help.p_console')}</li>
          <li>{t('help.p_problems')}</li>
          <li>{t('help.p_ir')}</li>
          <li>{t('help.p_bytecode')}</li>
          <li>{t('help.p_c')}</li>
          <li>{t('help.p_repl')}</li>
        </ul>
        <h3>{t('help.about')}</h3>
        <p>{t('help.about_text')}</p>
      </div>
    </Dialog>
  );
}

// ---- rename, share -----------------------------------------------------------------------------------------------

function RenameDialog({ onClose }) {
  const data = useStore(runtime, (s) => s.dialogData);
  const [name, setName] = useState(data?.placeholder ?? '');
  const ok = /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
  return (
    <Dialog
      title={t('rename.title')}
      onClose={onClose}
      footer={
        <>
          <span class="spacer" />
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button primary disabled={!ok} onClick={() => commitRename(name)}>
            {t('rename.apply')}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (ok) commitRename(name);
        }}
      >
        <Field label={t('rename.new_name')} hint={t('rename.hint')}>
          <FocusInput value={name} onInput={(e) => setName(e.currentTarget.value)} spellcheck={false} />
        </Field>
      </form>
    </Dialog>
  );
}

function ShareDialog({ onClose }) {
  const data = useStore(runtime, (s) => s.dialogData);
  const ref = useRef();
  useEffect(() => ref.current?.select(), []);
  return (
    <Dialog title={t('share.title')} onClose={onClose}>
      <p>{t('share.text')}</p>
      <textarea class="mono" ref={ref} readOnly rows={5} value={data?.url ?? ''} onFocus={(e) => e.currentTarget.select()} />
    </Dialog>
  );
}

// ---- command palette -----------------------------------------------------------------------------------------------

export function Palette({ onClose }) {
  const [q, setQ] = useState('');
  const [i, setI] = useState(0);
  const commands = useStore(runtime, (s) => s.commands) ?? [];
  const input = useRef();
  useEffect(() => {
    const prev = document.activeElement;
    input.current?.focus();
    const key = (e) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('keydown', key);
      prev?.focus?.();
    };
  }, []);
  const list = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return commands.filter((c) => !needle || c.title.toLowerCase().includes(needle));
  }, [q, commands]);
  useEffect(() => setI(0), [q]);
  const go = (c) => {
    onClose();
    setTimeout(() => c.run(), 0);
  };
  return (
    <div class="backdrop palette-backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div class="palette" role="dialog" aria-modal="true" aria-label={t('palette.title')}>
        <div class="palette-input">
          <Icon name="search" />
          <input
            ref={input}
            value={q}
            placeholder={t('palette.placeholder')}
            onInput={(e) => setQ(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose();
              else if (e.key === 'ArrowDown') (e.preventDefault(), setI((i + 1) % Math.max(1, list.length)));
              else if (e.key === 'ArrowUp') (e.preventDefault(), setI((i + list.length - 1) % Math.max(1, list.length)));
              else if (e.key === 'Enter' && list[i]) (e.preventDefault(), go(list[i]));
            }}
            spellcheck={false}
          />
        </div>
        <ul class="palette-list" role="listbox">
          {list.map((c, k) => (
            <li role="option" aria-selected={k === i} class={k === i ? 'is-active' : ''} onPointerMove={() => setI(k)} onClick={() => go(c)}>
              <span>{c.title}</span>
              {c.hint && <kbd>{c.hint}</kbd>}
            </li>
          ))}
          {list.length === 0 && <li class="empty">{t('palette.none')}</li>}
        </ul>
      </div>
    </div>
  );
}
