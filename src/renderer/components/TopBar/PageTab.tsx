// One tab of a capsule: the top bar's page tabs (PageTabs.tsx), Thread
// view's drawer selector and an agent tile's view picker
// (SessionView/ThreadDrawerSelector.tsx). Glass draws
// the capsule and its 24px circles from `data-page-tabs` / `data-page-tab`
// (styles/glass/overrides.css).

import type { ButtonHTMLAttributes, ReactNode, Ref } from 'react'

/** What a tab badge shows: nothing, a number, or `?` (unknown). */
export function badgeText(badge: number | '?' | undefined): string | null {
  if (badge === '?') return '?'
  if (badge === undefined || badge <= 0) return null
  return badge > 99 ? '99+' : String(badge)
}

/** Extra button props (a ref, `disabled`, `tabIndex`, keys, data-*) pass
 *  through. */
export function PageTab({
  selected,
  onSelect,
  badge,
  badgeClass,
  badgeStale = false,
  badgeTitle,
  title,
  ariaLabel,
  children,
  role,
  ...button
}: Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onClick' | 'title' | 'children' | 'className' | 'style'> & {
  ref?: Ref<HTMLButtonElement>
} & {
  selected: boolean
  onSelect: () => void
  /** A count (0 hides) or `?` when the server cannot give one. */
  badge?: number | '?'
  badgeClass?: string
  /** Dim the badge: the number may be out of date, or is unknown. */
  badgeStale?: boolean
  /** Tooltip on the badge saying why it is dimmed. */
  badgeTitle?: string
  title: string
  /** Accessible name when the tab shows only an icon. */
  ariaLabel?: string
  children: ReactNode
}): React.JSX.Element {
  const text = badgeText(badge)
  return (
    <button
      type="button"
      {...button}
      onClick={onSelect}
      data-page-tab=""
      data-selected={selected ? '' : undefined}
      className={`relative flex h-6 items-center justify-center px-2 text-[11px] font-medium transition-colors ${
        selected
          ? 'text-[var(--color-text-primary)] bg-white/[0.08] shadow-[inset_0_-2px_0_var(--color-accent)]'
          : 'text-[var(--color-text-secondary)] enabled:hover:bg-[var(--color-bg-elevated)] enabled:hover:text-[var(--color-text-primary)]'
      } disabled:cursor-not-allowed disabled:opacity-50`}
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag'
      }}
      title={title}
      aria-label={ariaLabel}
      role={role}
      // A radio (a view switch, ThreadDrawerSelector's `switch` mode) says
      // its state with `aria-checked`, passed through; never both.
      aria-pressed={role === 'radio' ? undefined : selected}
    >
      {children}
      {text !== null && (
        <span
          data-testid="page-tab-badge"
          data-stale={badgeStale ? 'true' : 'false'}
          title={badgeTitle}
          className={`absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] flex items-center justify-center text-[8px] font-bold text-[var(--color-on-accent)] rounded-[var(--radius-badge)] px-0.5 ${badgeClass ?? 'bg-[var(--color-accent)]'}${badgeStale ? ' opacity-50' : ''}`}
        >
          {text}
        </span>
      )}
    </button>
  )
}
