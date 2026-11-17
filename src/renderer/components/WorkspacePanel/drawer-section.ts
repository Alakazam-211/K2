// The drawers' collapsible section headers (Files: Environment, Workspace
// Resources, AI Config; Workspace: Heartbeats, Connected Agents, Apps,
// Published, API, Worktrees) share one hover fill: the palette's wash-1,
// a white wash on dark palettes and a dark one on light palettes. Every
// header reads its class from here, so none drifts to its own fill.
export const DRAWER_SECTION_HOVER = 'hover:bg-[var(--color-wash-1)]'

/** A Workspace drawer section header: a full-width bar, 11px label. */
export const DRAWER_SECTION_HEADER =
  `w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag ${DRAWER_SECTION_HOVER} transition-colors`

/** A Files drawer section header: 35px tall, small caps label. */
export const FILES_SECTION_HEADER =
  `w-full h-[35px] px-3 flex items-center gap-1.5 text-left text-[10px] uppercase tracking-wider font-semibold text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] ${DRAWER_SECTION_HOVER} transition-colors`
