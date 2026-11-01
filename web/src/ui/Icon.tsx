import type { JSX } from 'preact';
import s from './Icon.module.scss';

const PATHS = {
  // run / stop: solid, the only filled shapes
  play: <path d="M8.5 5.6v12.8a.9.9 0 0 0 1.4.8l10-6.4a.9.9 0 0 0 0-1.5l-10-6.4a.9.9 0 0 0-1.4.7z" fill="currentColor" />,
  stop: <rect x="6.5" y="6.5" width="11" height="11" rx="2" fill="currentColor" />,
  // check the project: a tick in a ring
  check: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M8 12.4l2.8 2.8L16.2 9.6" />
    </>
  ),
  // format: lines that step in, with the arrow that does it
  format: <path d="M4 5.5h16M12 10h8M12 14h8M4 18.5h16M4 9.5l3 2.5-3 2.5" />,
  file: <path d="M6.5 3h7.5l4.5 4.5V21h-12zM14 3v4.5h4.5" />,
  book: <path d="M4 5.5h5.5a2.5 2.5 0 0 1 2.5 2.5v11a2.5 2.5 0 0 0-2.5-2.5H4zM20 5.5h-5.5A2.5 2.5 0 0 0 12 8v11a2.5 2.5 0 0 1 2.5-2.5H20z" />,
  list: <path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01" />,
  // layout toggles: one frame, one divider each, at the same distance from the edge
  sidebar: (
    <>
      <rect x="3" y="3.5" width="18" height="17" rx="2" />
      <path d="M9.5 3.5v17" />
    </>
  ),
  'panel-bottom': (
    <>
      <rect x="3" y="3.5" width="18" height="17" rx="2" />
      <path d="M3 14.5h18" />
    </>
  ),
  'panel-right': (
    <>
      <rect x="3" y="3.5" width="18" height="17" rx="2" />
      <path d="M14.5 3.5v17" />
    </>
  ),
  // settings: sliders
  gear: (
    <>
      <path d="M4 7h9M19 7h1M4 17h1M11 17h9" />
      <circle cx="16" cy="7" r="2.6" />
      <circle cx="8" cy="17" r="2.6" />
    </>
  ),
  help: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M9.6 9.7a2.5 2.5 0 1 1 3.6 2.2c-.8.4-1.2 1-1.2 1.8M12 17h.01" />
    </>
  ),
  sun: (
    <>
      <circle cx="12" cy="12" r="3.8" />
      <path d="M12 3.5v2M12 18.5v2M3.5 12h2M18.5 12h2M6 6l1.4 1.4M16.6 16.6L18 18M6 18l1.4-1.4M16.6 7.4L18 6" />
    </>
  ),
  moon: <path d="M20 14.2A8 8 0 0 1 9.8 4a8 8 0 1 0 10.2 10.2z" />,
  monitor: (
    <>
      <rect x="3" y="4" width="18" height="12.5" rx="2" />
      <path d="M8.5 20h7M12 16.5V20" />
    </>
  ),
  share: <path d="M12 14.5V3.5M8 7.5l4-4 4 4M6 11H5.5v9.5h13V11H18" />,
  download: <path d="M12 3.5v11M8 10.5l4 4 4-4M5 20.5h14" />,
  upload: <path d="M12 14.5v-11M8 7.5l4-4 4 4M5 20.5h14" />,
  plus: <path d="M12 5v14M5 12h14" />,
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  terminal: <path d="M4.5 7l5 5-5 5M12.5 17.5h7" />,
  alert: <path d="M12 4l9 16H3zM12 10v4M12 17h.01" />,
  chevron: <path d="M9.5 6l6 6-6 6" />,
  copy: (
    <>
      <rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2" />
      <path d="M15.5 8.5V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v7.5a2 2 0 0 0 2 2h2.5" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="M16 16l4.5 4.5" />
    </>
  ),
  trash: <path d="M4.5 7h15M9.5 7V4.5h5V7M6.5 7l.8 12.5h9.4L17.5 7M10 11v5M14 11v5" />,
  edit: <path d="M4 20h4L19.5 8.5l-4-4L4 16zM13 7l4 4" />,
  flag: <path d="M6 21V4M6 5h11.5l-2.5 4 2.5 4H6" />,
  more: <path d="M5 12h.01M12 12h.01M19 12h.01" />,
  command: <path d="M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3" />,
  reset: <path d="M4 12a8 8 0 1 0 2.4-5.7M4 4v5h5" />,
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v6M12 7.5h.01" />
    </>
  ),
  // the sections of the settings
  palette: (
    <>
      <path d="M12 3a9 9 0 1 0 0 18c1.1 0 1.8-.8 1.8-1.7 0-.5-.2-.9-.5-1.2s-.5-.7-.5-1.2c0-.9.8-1.6 1.7-1.6H17a4 4 0 0 0 4-4c0-4.4-4-8.3-9-8.3z" />
      <path d="M7.5 11h.01M10 7.5h.01M14.5 7.5h.01" />
    </>
  ),
  code: <path d="M8 7l-5 5 5 5M16 7l5 5-5 5M13.5 5l-3 14" />,
  cpu: (
    <>
      <rect x="7" y="7" width="10" height="10" rx="1.5" />
      <path d="M10 3.5V7M14 3.5V7M10 17v3.5M14 17v3.5M3.5 10H7M3.5 14H7M17 10h3.5M17 14h3.5" />
    </>
  ),
  box: <path d="M12 3l8 4.5v9L12 21l-8-4.5v-9zM4 7.5l8 4.5 8-4.5M12 12v9" />,
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3c2.7 2.5 4 5.5 4 9s-1.3 6.5-4 9c-2.7-2.5-4-5.5-4-9s1.3-6.5 4-9z" />
    </>
  ),
} satisfies Record<string, JSX.Element>;

export type IconName = keyof typeof PATHS;

interface IconProps {
  readonly name: IconName;
  readonly size?: number | undefined;
  readonly class?: string | undefined;
}

/** A line icon, 24x24, drawn with the text color. */
export function Icon({ name, size = 16, class: cls }: IconProps): JSX.Element {
  return (
    <svg
      class={cls ? `${s.icon} ${cls}` : s.icon}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      {PATHS[name]}
    </svg>
  );
}
