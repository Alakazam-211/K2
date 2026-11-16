// The E3 tools' line glyphs (the mock's set, editors-notes.md): find,
// replace, the format buttons and the / menu's blocks. currentColor, 24 grid.

const PATHS: Record<string, { d: React.ReactNode; w?: number }> = {
  search: { d: <><circle cx="11" cy="11" r="6.5" /><path d="M16 16l4.5 4.5" /></>, w: 2.2 },
  replace: { d: <path d="M4 7h11l-3-3M20 17H9l3 3" />, w: 2 },
  up: { d: <path d="M6 15l6-6 6 6" />, w: 2.2 },
  down: { d: <path d="M6 9l6 6 6-6" />, w: 2.2 },
  close: { d: <path d="M6 6l12 12M18 6L6 18" />, w: 2.2 },
  bold: { d: <path d="M7 5h6a3.5 3.5 0 0 1 0 7H7zM7 12h7a3.5 3.5 0 0 1 0 7H7z" />, w: 2.4 },
  italic: { d: <path d="M10 5h8M6 19h8M14.5 5l-5 14" />, w: 2.2 },
  code: { d: <path d="M8.5 8L4.5 12l4 4M15.5 8l4 4-4 4" />, w: 2.2 },
  link: { d: <><path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1" /><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" /></>, w: 2 },
  heading: { d: <path d="M6 5v14M18 5v14M6 12h12" />, w: 2.4 },
  list: { d: <><path d="M9.5 6.5H20M9.5 12H20M9.5 17.5H20" /><circle cx="5" cy="6.5" r="1" fill="currentColor" /><circle cx="5" cy="12" r="1" fill="currentColor" /><circle cx="5" cy="17.5" r="1" fill="currentColor" /></> },
  bullet: { d: <><path d="M9.5 6.5H20M9.5 12H20M9.5 17.5H20" /><circle cx="5" cy="6.5" r="1" fill="currentColor" /><circle cx="5" cy="12" r="1" fill="currentColor" /><circle cx="5" cy="17.5" r="1" fill="currentColor" /></> },
  task: { d: <><rect x="3.5" y="4.5" width="7" height="7" rx="1.5" /><path d="M5.3 8l1.3 1.3L9 6.8M13.5 8H20M13.5 16H20" /><rect x="3.5" y="12.5" width="7" height="7" rx="1.5" /></> },
  quote: { d: <path d="M5 6v12M9 8h10M9 12h10M9 16h6" />, w: 2 },
  table: { d: <><rect x="3.5" y="4.5" width="17" height="15" rx="1.5" /><path d="M3.5 9.5h17M3.5 14.5h17M10 9.5v10" /></> },
  divider: { d: <><path d="M3.5 12h17" /><path d="M7 6.5h10M7 17.5h10" opacity=".45" /></>, w: 2 },
  image: { d: <><rect x="3.5" y="4.5" width="17" height="15" rx="2" /><circle cx="9" cy="10" r="1.6" /><path d="M20.5 16l-5-5-8.5 8.5" /></> },
  outline: { d: <path d="M4 6h16M8 12h12M8 18h12" />, w: 2.2 },
}

export function ToolGlyph({ name, size = 15 }: { name: string; size?: number }): React.JSX.Element {
  const p = PATHS[name] ?? PATHS.close
  return (
    <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={p.w ?? 1.8} strokeLinecap="round" strokeLinejoin="round">
      {p.d}
    </svg>
  )
}
