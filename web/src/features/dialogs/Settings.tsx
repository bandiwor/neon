import type { JSX } from 'preact';
import { t, type MessageKey } from '@/core/i18n';
import { DEFAULT_SETTINGS, runtime, settings } from '@/core/state/stores';
import type { Accent, OptLevel, PanelPosition, Theme } from '@/core/state/types';
import { useStore } from '@/core/state/useStore';
import { Button, Dialog, Field, Segmented, Switch, cx } from '@/ui';
import u from '@/styles/utils.module.scss';
import s from './Settings.module.scss';

const ACCENTS: readonly { readonly id: Accent; readonly color: string }[] = [
  { id: 'ember', color: '#c2410c' },
  { id: 'moss', color: '#2f7d4f' },
  { id: 'azure', color: '#2563b8' },
  { id: 'graphite', color: '#52524c' },
];

const OPT_LEVELS: readonly OptLevel[] = ['default', 'none', 'no-inline', 'no-peephole'];

export function SettingsDialog(): JSX.Element {
  const st = useStore(settings);
  const close = (): void => runtime.set({ dialog: null });
  return (
    <Dialog
      title={t('settings.title')}
      onClose={close}
      wide
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
      <div class={s.grid}>
        <section>
          <h3 class={s.heading}>{t('settings.appearance')}</h3>
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
          <Field label={t('settings.font_size')}>
            <div class={s.range}>
              <input type="range" min="11" max="22" step="1" value={st.fontSize} onInput={(e) => settings.set({ fontSize: Number(e.currentTarget.value) })} />
              <span class={u.muted}>{st.fontSize} px</span>
            </div>
          </Field>
          <Switch checked={st.wrap} onChange={(wrap) => settings.set({ wrap })}>
            {t('settings.wrap')}
          </Switch>
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
        </section>
        <section>
          <h3 class={s.heading}>{t('settings.run')}</h3>
          <Field label={t('settings.opt')} hint={t('settings.opt_hint')}>
            <select value={st.opt} onChange={(e) => settings.set({ opt: e.currentTarget.value as OptLevel })}>
              {OPT_LEVELS.map((o) => (
                <option key={o} value={o}>
                  {t(`opt.${o}` satisfies MessageKey)}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t('settings.max_steps')} hint={t('settings.max_steps_hint')}>
            <input type="number" min="0" step="1000000" value={st.maxSteps} onInput={(e) => settings.set({ maxSteps: Math.max(0, Number(e.currentTarget.value) || 0) })} />
          </Field>
          <Field label={t('settings.timeout')} hint={t('settings.timeout_hint')}>
            <input type="number" min="0" max="600" value={st.timeout} onInput={(e) => settings.set({ timeout: Math.max(0, Number(e.currentTarget.value) || 0) })} />
          </Field>
          <Field label={t('settings.gc_limit')} hint={t('settings.gc_limit_hint')}>
            <input value={st.gcLimit} placeholder="64m" spellcheck={false} onInput={(e) => settings.set({ gcLimit: e.currentTarget.value })} />
          </Field>
          <Switch checked={st.gcStress} onChange={(gcStress) => settings.set({ gcStress })}>
            {t('settings.gc_stress')}
          </Switch>
          <Switch checked={st.stats} onChange={(stats) => settings.set({ stats })}>
            {t('settings.stats')}
          </Switch>
        </section>
      </div>
    </Dialog>
  );
}
