// The left nav's collapsible-section pieces (the Active and Pinned
// headers), shared so other collapsible sections (the Design notes tray)
// speak the same design language instead of copying the classes.

import type { ReactNode } from 'react'

/** A section header row's hover (the nav's faint white wash). */
export const NAV_SECTION_HOVER_CLASS = 'hover:bg-white/[0.02] transition-colors'

/** The section name: small, uppercase, letter-spaced, muted. */
export function NavSectionLabel({ children }: { children: ReactNode }): React.JSX.Element {
  return (
    <span className="text-[10px] font-semibold tracking-wider text-[var(--color-text-muted)] uppercase" data-nav-section-label="">
      {children}
    </span>
  )
}

/** The count tag beside a section name (Active's "3"). */
export function NavCountTag({ count, testId }: { count: number; testId?: string }): React.JSX.Element {
  return (
    <span
      className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono"
      data-nav-count-tag=""
      data-testid={testId}
    >
      {count}
    </span>
  )
}

/** The collapse chevron: points right when collapsed, down when open, and
 *  turns between the two. */
export function NavSectionChevron({ collapsed }: { collapsed: boolean }): React.JSX.Element {
  return (
    <svg
      className="w-2.5 h-2.5 text-[var(--color-text-muted)] flex-shrink-0"
      style={{ transition: 'transform 0.2s ease', transform: collapsed ? 'rotate(0deg)' : 'rotate(90deg)' }}
      fill="none"
      viewBox="0 0 24 24"
      stroke="currentColor"
      strokeWidth={2.5}
      aria-hidden="true"
      data-nav-section-chevron={collapsed ? 'collapsed' : 'open'}
    >
      <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
    </svg>
  )
}
