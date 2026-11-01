import type { ComponentChildren, JSX } from 'preact';
import { useState } from 'preact/hooks';
import { t, type MessageKey } from '@/core/i18n';
import { DEFAULT_OPT, DEFAULT_SETTINGS, runtime, settings } from '@/core/state/stores';
import type { Accent, LineHeight, OptFlags, PanelPosition, SyntaxStyle, Theme } from '@/core/state/types';
import { useStore } from '@/core/state/useStore';
import { Button, Dialog, Field, Icon, Segmented, Switch, cx, type IconName } from '@/ui';
import u from '@/styles/utils.module.scss';
import s from './Settings.module.scss';

type Section = 'appearance' | 'editor' | 'compiler' | 'vm' | 'program';

/** The sections of the dialog: add one here (and a `Pane`) to extend it. */
const SECTIONS: readonly { readonly id: Section; readonly icon: IconName }[] = [
  { id: 'appearance', icon: 'palette' },
  { id: 'editor', icon: 'code' },
  { id: 'compiler', icon: 'cpu' },
  { id: 'vm', icon: 'box' },
  { id: 'program', icon: 'terminal' },
];

const ACCENTS: readonly { readonly id: Accent; readonly color: string }[] = [
  { id: 'ember', color: '#c2410c' },
  { id: 'moss', color: '#2f7d4f' },
  { id: 'azure', color: '#2563b8' },
  { id: 'graphite', color: '#52524c' },
];

/** The boolean passes of the optimizer, in pipeline order. */
const PASSES: readonly (keyof OptFlags & ('fold' | 'dce' | 'simplify' | 'licm' | 'rotate' | 'unroll' | 'inline' | 'peephole'))[] = [
  'fold',
  'simplify',
  'dce',
  'licm',
  'rotate',
  'unroll',
  'inline',
  'peephole',
];

const number = (e: JSX.TargetedEvent<HTMLInputElement>, min = 0): number => Math.max(min, Math.trunc(Number(e.currentTarget.value)) || 0);

function Pair({ children }: { readonly children: ComponentChildren }): JSX.Element {
  return <div class={s.pair}>{children}</div>;
}

function Appearance(): JSX.Element {
  const st = useStore(settings);
  return (
    <>
      <Field label={t('settings.theme')}>
        <Segmented<Theme>
          value={st.theme}
          onChange={(theme) => settings.set({ theme })}
          label={t('settings.theme')}
          options={[
            { value: 'system', icon: 'monitor', label: t('theme.system') },
            { value: 'light', icon: 'sun', label: t('theme.light') },
            { value: 'dark', icon: 'moon', label: t('theme.dark') },
          ]}
        />
      </Field>
      <Field label={t('settings.accent')}>
        <div class={s.swatches} role="radiogroup" aria-label={t('settings.accent')}>
          {ACCENTS.map((a) => (
            <button
              key={a.id}
              type="button"
              role="radio"
              aria-checked={st.accent === a.id}
              class={cx(s.swatch, st.accent === a.id && s.swatchOn)}
              style={{ '--swatch': a.color }}
              onClick={() => settings.set({ accent: a.id })}
              title={t(`accent.${a.id}` satisfies MessageKey)}
            >
              <span class={s.swatchDot} />
              <span>{t(`accent.${a.id}` satisfies MessageKey)}</span>
            </button>
          ))}
        </div>
      </Field>
      <Field label={t('settings.panel_pos')}>
        <Segmented<PanelPosition>
          value={st.panelPos}
          onChange={(panelPos) => settings.set({ panelPos })}
          label={t('settings.panel_pos')}
          options={[
            { value: 'auto', label: t('panelpos.auto') },
            { value: 'right', icon: 'panel-right', label: t('panelpos.right') },
            { value: 'bottom', icon: 'panel-bottom', label: t('panelpos.bottom') },
          ]}
        />
      </Field>
      <Field label={t('settings.lang')}>
        <select value={st.lang} onChange={(e) => settings.set({ lang: e.currentTarget.value as 'ru' })}>
          <option value="ru">{t('lang.ru')}</option>
        </select>
      </Field>
    </>
  );
}

function Editor(): JSX.Element {
  const st = useStore(settings);
  return (
    <>
      <Field label={t('settings.syntax')}>
        <Segmented<SyntaxStyle>
          value={st.syntax}
          onChange={(syntax) => settings.set({ syntax })}
          label={t('settings.syntax')}
          options={(['neon', 'quiet', 'contrast'] as const).map((value) => ({ value, label: t(`syntax.${value}` satisfies MessageKey) }))}
        />
      </Field>
      <Field label={t('settings.font_size')}>
        <div class={s.range}>
          <input type="range" min="11" max="22" step="1" value={st.fontSize} onInput={(e) => settings.set({ fontSize: Number(e.currentTarget.value) })} />
          <span class={u.muted}>{st.fontSize} px</span>
        </div>
      </Field>
        <Field label={t('settings.line_height')}>
          <Segmented<LineHeight>
            value={st.lineHeight}
            onChange={(lineHeight) => settings.set({ lineHeight })}
            label={t('settings.line_height')}
            options={(['tight', 'normal', 'loose'] as const).map((value) => ({ value, label: t(`lh.${value}` satisfies MessageKey) }))}
          />
        </Field>
        <Field label={t('settings.tab_size')}>
          <Segmented<'2' | '4' | '8'>
            value={String(st.tabSize) as '2' | '4' | '8'}
            onChange={(v) => settings.set({ tabSize: Number(v) })}
            label={t('settings.tab_size')}
            options={(['2', '4', '8'] as const).map((value) => ({ value, label: value }))}
          />
        </Field>
      <div class={s.switches}>
        <Switch checked={st.wrap} onChange={(wrap) => settings.set({ wrap })}>
          {t('settings.wrap')}
        </Switch>
        <Switch checked={st.lineNumbers} onChange={(lineNumbers) => settings.set({ lineNumbers })}>
          {t('settings.line_numbers')}
        </Switch>
        <Switch checked={st.activeLine} onChange={(activeLine) => settings.set({ activeLine })}>
          {t('settings.active_line')}
        </Switch>
        <Switch checked={st.folding} onChange={(folding) => settings.set({ folding })}>
          {t('settings.folding')}
        </Switch>
        <Switch checked={st.closeBrackets} onChange={(closeBrackets) => settings.set({ closeBrackets })}>
          {t('settings.close_brackets')}
        </Switch>
        <Switch checked={st.matchBrackets} onChange={(matchBrackets) => settings.set({ matchBrackets })}>
          {t('settings.match_brackets')}
        </Switch>
      </div>
    </>
  );
}

function Compiler(): JSX.Element {
  const opt = useStore(settings, (st) => st.opt);
  const set = (patch: Partial<OptFlags>): void => settings.set({ opt: { ...opt, ...patch } });
  const all = (on: boolean): void => set(Object.fromEntries(PASSES.map((p) => [p, on])) as Partial<OptFlags>);
  return (
    <>
      <div class={s.bar}>
        <span class={s.barTitle}>{t('settings.passes')}</span>
        <Button onClick={() => all(true)}>{t('settings.all_on')}</Button>
        <Button onClick={() => all(false)}>{t('settings.all_off')}</Button>
      </div>
      <div class={s.switches}>
        {PASSES.map((p) => (
          <Switch key={p} checked={opt[p]} onChange={(on) => set({ [p]: on })}>
            {t(`opt.${p}` satisfies MessageKey)}
          </Switch>
        ))}
      </div>
      <Pair>
        <Field label={t('opt.inline_threshold')}>
          <input type="number" min="0" disabled={!opt.inline} value={opt.inlineThreshold} onInput={(e) => set({ inlineThreshold: number(e) })} />
        </Field>
        <Field label={t('opt.max_regs')}>
          <input type="number" min="8" disabled={!opt.inline} value={opt.maxRegs} onInput={(e) => set({ maxRegs: number(e, 8) })} />
        </Field>
      </Pair>
      <Button icon="reset" onClick={() => set({ ...DEFAULT_OPT })}>
        {t('settings.defaults')}
      </Button>
    </>
  );
}

function Vm(): JSX.Element {
  const st = useStore(settings);
  return (
    <>
      <Pair>
        <Field label={t('settings.max_steps')} hint={t('settings.zero_is_off')}>
          <input type="number" min="0" step="1000000" value={st.maxSteps} onInput={(e) => settings.set({ maxSteps: number(e) })} />
        </Field>
        <Field label={t('settings.max_frames')} hint={t('settings.zero_is_off')}>
          <input type="number" min="0" step="1000" value={st.maxFrames} onInput={(e) => settings.set({ maxFrames: number(e) })} />
        </Field>
        <Field label={t('settings.timeout')} hint={t('settings.zero_is_off')}>
          <input type="number" min="0" max="600" value={st.timeout} onInput={(e) => settings.set({ timeout: number(e) })} />
        </Field>
        <Field label={t('settings.gc_limit')}>
          <input value={st.gcLimit} placeholder="64m" spellcheck={false} onInput={(e) => settings.set({ gcLimit: e.currentTarget.value })} />
        </Field>
      </Pair>
      <div class={s.switches}>
        <Switch checked={st.gcStress} onChange={(gcStress) => settings.set({ gcStress })}>
          {t('settings.gc_stress')}
        </Switch>
        <Switch checked={st.stats} onChange={(stats) => settings.set({ stats })}>
          {t('settings.stats')}
        </Switch>
      </div>
    </>
  );
}

function Program(): JSX.Element {
  const st = useStore(settings);
  return (
    <>
      <Field label={t('settings.args')}>
        <input value={st.args} placeholder={t('settings.args_placeholder')} spellcheck={false} onInput={(e) => settings.set({ args: e.currentTarget.value })} />
      </Field>
      <Field label={t('settings.stdin')}>
        <textarea rows={6} value={st.stdin} spellcheck={false} onInput={(e) => settings.set({ stdin: e.currentTarget.value })} />
      </Field>
    </>
  );
}

const PANES: Readonly<Record<Section, () => JSX.Element>> = { appearance: Appearance, editor: Editor, compiler: Compiler, vm: Vm, program: Program };

export function SettingsDialog(): JSX.Element {
  const [section, setSection] = useState<Section>('appearance');
  const close = (): void => runtime.set({ dialog: null });
  const Pane = PANES[section];
  return (
    <Dialog
      title={t('settings.title')}
      onClose={close}
      wide
      flush
      footer={
        <>
          <Button icon="reset" onClick={() => confirm(t('settings.confirm_reset')) && settings.set({ ...DEFAULT_SETTINGS })}>
            {t('settings.reset')}
          </Button>
          <span class={u.spacer} />
          <Button primary onClick={close}>
            {t('common.close')}
          </Button>
        </>
      }
    >
      <div class={s.layout}>
        <nav class={s.nav} aria-label={t('settings.title')}>
          {SECTIONS.map((x) => (
            <button key={x.id} type="button" class={cx(s.navItem, x.id === section && s.navOn)} aria-current={x.id === section ? 'page' : undefined} onClick={() => setSection(x.id)}>
              <Icon name={x.icon} />
              <span>{t(`settings.s_${x.id}` satisfies MessageKey)}</span>
            </button>
          ))}
        </nav>
        <div class={s.pane}>
          <Pane />
        </div>
      </div>
    </Dialog>
  );
}
