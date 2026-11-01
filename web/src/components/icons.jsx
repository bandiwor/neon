// Line icons, 24x24, drawn with the text color.
const paths = {
  play: <path d="M7 4.5v15l12-7.5z" />,
  stop: <rect x="6" y="6" width="12" height="12" rx="1" />,
  check: <path d="M4.5 12.5l5 5L19.5 7" />,
  format: <path d="M4 6h16M4 10h10M4 14h16M4 18h10" />,
  file: <path d="M7 3h7l4 4v14H7zM14 3v4h4" />,
  book: <path d="M3.5 5.5h6a2.5 2.5 0 0 1 2.5 2.5v11a2.5 2.5 0 0 0-2.5-2.5h-6zM20.5 5.5h-6A2.5 2.5 0 0 0 12 8v11a2.5 2.5 0 0 1 2.5-2.5h6z" />,
  list: <path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01" />,
  sidebar: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <path d="M9.5 4v16" />
    </>
  ),
  'panel-bottom': (
    <>
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <path d="M3 14h18" />
    </>
  ),
  'panel-right': (
    <>
      <rect x="3" y="4" width="18" height="16" rx="1.5" />
      <path d="M14.5 4v16" />
    </>
  ),
  gear: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" />
    </>
  ),
  help: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M9.6 9.6a2.5 2.5 0 1 1 3.6 2.2c-.8.4-1.2 1-1.2 1.8M12 17h.01" />
    </>
  ),
  sun: (
    <>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6L7 7M17 17l1.4 1.4M5.6 18.4L7 17M17 7l1.4-1.4" />
    </>
  ),
  moon: <path d="M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z" />,
  monitor: (
    <>
      <rect x="3" y="4" width="18" height="12" rx="1.5" />
      <path d="M8 20h8M12 16v4" />
    </>
  ),
  share: <path d="M12 15V3M7.5 7.5L12 3l4.5 4.5M5 13v7h14v-7" />,
  download: <path d="M12 3v12M7 10.5l5 4.5 5-4.5M5 20h14" />,
  upload: <path d="M12 15V3M7 7.5L12 3l5 4.5M5 20h14" />,
  plus: <path d="M12 5v14M5 12h14" />,
  x: <path d="M6 6l12 12M18 6L6 18" />,
  terminal: <path d="M4 6l6 6-6 6M12 18h8" />,
  alert: <path d="M12 4l9 16H3zM12 10v4M12 17h.01" />,
  chevron: <path d="M9 6l6 6-6 6" />,
  copy: (
    <>
      <rect x="8" y="8" width="12" height="12" rx="1.5" />
      <path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      <path d="M16 16l4.5 4.5" />
    </>
  ),
  trash: <path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13" />,
  edit: <path d="M4 20h4L19 9l-4-4L4 16zM13 7l4 4" />,
  flag: <path d="M6 21V4M6 5h11l-2 4 2 4H6" />,
  more: <path d="M5 12h.01M12 12h.01M19 12h.01" />,
  command: <path d="M9 9V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3z" />,
  reset: <path d="M4 12a8 8 0 1 0 2.5-5.8M4 4v5h5" />,
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v6M12 7.5h.01" />
    </>
  ),
};

export function Icon({ name, size = 16, class: cls = '' }) {
  return (
    <svg
      class={`icon ${cls}`}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}
