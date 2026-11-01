import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { Dialogs } from '@/features/dialogs';
import { Editor } from '@/features/editor';
import { Panel } from '@/features/panel';
import { Sidebar } from '@/features/sidebar';
import { t } from '@/core/i18n';
import { setActive } from '@/core/project/files';
import { project, runtime, settings, ui } from '@/core/state/stores';
import { useStore } from '@/core/state/useStore';
import { Icon, Resizer, cx } from '@/ui';
import { commands } from './commands';
import { Header } from './Header';
import { installShortcuts } from './shortcuts';
import { StatusBar } from './StatusBar';
import { Toast } from './Toast';
import s from './App.module.scss';

const NARROW = '(max-width: 980px)';

function useNarrow(): boolean {
  const [narrow, setNarrow] = useState(() => matchMedia(NARROW).matches);
  useEffect(() => {
    const q = matchMedia(NARROW);
    const on = (): void => setNarrow(q.matches);
    q.addEventListener('change', on);
    return () => q.removeEventListener('change', on);
  }, []);
  return narrow;
}

export function App(): JSX.Element {
  const st = useStore(settings);
  const sidebar = useStore(ui, (v) => v.sidebar);
  const panelOpen = useStore(ui, (v) => v.panelOpen);
  const narrow = useNarrow();
  const work = useRef<HTMLDivElement>(null);
  // the palette lists the commands in the current language
  const list = useMemo(() => commands(), [st.lang]);

  // theme, accent, font size
  useEffect(() => {
    const root = document.documentElement;
    root.dataset['theme'] = st.theme;
    root.dataset['accent'] = st.accent;
    root.style.setProperty('--code-size', `${st.fontSize}px`);
    root.lang = st.lang;
  }, [st.theme, st.accent, st.fontSize, st.lang]);

  useEffect(installShortcuts, []);

  const position = st.panelPos === 'auto' ? (narrow ? 'bottom' : 'right') : st.panelPos;

  return (
    <div class={cx(s.app, position === 'bottom' ? s.panelBottom : s.panelRight)}>
      <Header />
      <div class={s.main}>
        {sidebar && <SidebarArea narrow={narrow} />}
        <div class={s.work} ref={work}>
          <EditorArea />
          {panelOpen && <PanelArea work={work} position={position} />}
        </div>
      </div>
      <StatusBar />
      <Dialogs commands={list} />
      <Toast />
    </div>
  );
}

function SidebarArea({ narrow }: { readonly narrow: boolean }): JSX.Element {
  const width = useStore(ui, (v) => v.sidebarWidth);
  const startWidth = useRef(width);
  return (
    <>
      <div class={cx(s.sidebar, narrow && s.overlay)} style={narrow ? undefined : { width: `${width}px` }}>
        <Sidebar />
        {!narrow && (
          <Resizer
            axis="x"
            edge="end"
            onDrag={(d) => ui.set({ sidebarWidth: Math.max(160, Math.min(480, startWidth.current + d)) })}
            onDone={() => (startWidth.current = ui.get().sidebarWidth)}
          />
        )}
      </div>
      {narrow && <div class={s.scrim} onClick={() => ui.set({ sidebar: false })} />}
    </>
  );
}

function EditorArea(): JSX.Element {
  const files = useStore(project, (p) => p.files);
  const active = useStore(project, (p) => p.active);
  const entry = useStore(project, (p) => p.entry);
  const problems = useStore(runtime, (r) => r.problems);
  return (
    <div class={s.editorArea}>
      <div class={s.fileTabs} role="tablist" aria-label={t('files.title')}>
        {files.map((f) => {
          const errors = (problems[f.name] ?? []).filter((d) => (d.severity ?? 1) === 1).length;
          return (
            <button
              key={f.name}
              type="button"
              role="tab"
              aria-selected={f.name === active}
              class={cx(s.fileTab, f.name === active && s.fileTabOn)}
              onClick={() => setActive(f.name)}
              title={f.name === entry ? t('files.entry') : f.name}
            >
              <span>{f.name}</span>
              {errors > 0 && <span class={s.errorDot} />}
              {f.name === entry && <Icon name="flag" size={12} class={s.entryFlag} />}
            </button>
          );
        })}
      </div>
      <Editor />
    </div>
  );
}

interface PanelAreaProps {
  readonly work: { readonly current: HTMLDivElement | null };
  readonly position: 'right' | 'bottom';
}

function PanelArea({ work, position }: PanelAreaProps): JSX.Element {
  const size = useStore(ui, (v) => v.panelSize);
  const start = useRef(size);
  const style = position === 'right' ? { width: `${size * 100}%` } : { height: `${size * 100}%` };
  return (
    <div class={s.panel} style={style}>
      <Resizer
        axis={position === 'right' ? 'x' : 'y'}
        onDrag={(d) => {
          const box = work.current?.getBoundingClientRect();
          if (!box) return;
          const total = position === 'right' ? box.width : box.height;
          ui.set({ panelSize: Math.max(0.15, Math.min(0.8, start.current - d / total)) });
        }}
        onDone={() => (start.current = ui.get().panelSize)}
      />
      <Panel />
    </div>
  );
}
