// SearchableAgentList — the searchable agent / workspace list shared by the
// Tickets + Wiki workspace filter (`Feedback/WorkspaceFilterDropdown`) and
// Home's Add Agent picker (`Home/HomeAddPanels`).
// prd-home-picker-and-remote-avatars-v1 P3/P4/P7, vs-live P25/P26/P35.
//
// What it draws: a search box, then sections (optional header with a color
// or accent bar and a count, an optional status slot), then rows (an agent
// image via `ProjectAvatar`, or a caller's glyph; the name; an optional
// detail; a check when the row is already taken). ↑/↓ walk the pickable
// rows across sections and clamp at the ends, Enter picks, and the
// keyboard row scrolls into view.
//
// It reads NO stores: callers build the sections (the window's focus groups
// mean nothing for another server's list). Filtering is the caller's, or
// pass `matches` to have the list hide rows that don't match the query.
// Esc is the caller's too: only with `onEscape` does the list stop the key
// (Tickets closes its popover before the page sees it); without it the key
// bubbles (Home's sidebar listener closes the picker).

import React, { useEffect, useId, useMemo, useRef, useState } from 'react'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'

/** A row's image: a workspace's `ProjectAvatar`, or a caller's glyph. */
export type AgentListAvatar =
  | {
      kind: 'workspace'
      path: string
      name: string
      color: string
      id?: string
      iconUrl: string | null
      /** False skips the connected server's icon lookup (another server's
       *  agent: its path means nothing here). */
      fetchIcon: boolean
    }
  | { kind: 'node'; node: React.ReactNode }

/** `pickable` rows take the click / Enter. `checked` (already taken) paints
 *  the accent check; `checked` and `disabled` are skipped by the arrow keys
 *  and ignore clicks. */
export type AgentListRowState = 'pickable' | 'checked' | 'disabled'

export interface AgentListRow {
  /** Unique across the whole list; what `onPick` receives. */
  value: string
  label: string
  /** Small muted text at the row's end (a count, a handle). */
  detail?: React.ReactNode
  avatar: AgentListAvatar
  state: AgentListRowState
  /** The caller's current value (Tickets' selected filter). */
  selected?: boolean
  title?: string
  /** Extra classes on the label (Tickets' italic Unlinked row). */
  labelClassName?: string
  /** More text the default `matches` searches (a path, a handle). */
  keywords?: string[]
}

export interface AgentListSection {
  key: string
  /** Header text; null = no header (a flat list, or loose rows on top). */
  label: string | null
  /** The header's color bar. */
  color?: string | null
  /** The header's accent bar (Tickets' Projects section). */
  accent?: boolean
  /** Header count; defaults to the rows shown. */
  count?: number
  /** Shown under the header (loading, offline, sign in…). A section with
   *  no rows left to show and no status is hidden. */
  status?: React.ReactNode
  rows: AgentListRow[]
}

export interface SearchableAgentListProps {
  sections: AgentListSection[]
  query: string
  onQuery: (query: string) => void
  onPick: (value: string) => void
  placeholder: string
  /** Shown when no section is left to show. */
  emptyText: string
  /** When given, Esc in the search box is stopped here and calls this
   *  (P35). Without it the key bubbles to the caller's own listener. */
  onEscape?: () => void
  /** Hide rows this rejects for a non-empty query. Omit when the caller
   *  filters its own sections. */
  matches?: (row: AgentListRow, query: string) => boolean
  /** Focus the search box on mount. */
  autoFocus?: boolean
  /** The listbox's accessible name. */
  ariaLabel: string
  /** Classes of the scrolling list body. */
  listClassName?: string
}

/** Case-insensitive substring match over the label and `keywords`; an
 *  empty query matches everything. */
export function matchesAgentRow(row: AgentListRow, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return [row.label, ...(row.keywords ?? [])].some((t) => t.toLowerCase().includes(q))
}

/** The sections as shown for `query`: rows filtered by `matches` (when
 *  given), and sections with no rows and no status dropped. */
export function visibleAgentSections(
  sections: AgentListSection[],
  query: string,
  matches?: (row: AgentListRow, query: string) => boolean,
): AgentListSection[] {
  const out: AgentListSection[] = []
  for (const s of sections) {
    const rows = matches && query.trim() ? s.rows.filter((r) => matches(r, query)) : s.rows
    if (rows.length === 0 && !s.status) continue
    out.push(rows === s.rows ? s : { ...s, rows })
  }
  return out
}

/** The values ↑/↓ walk, in paint order: pickable rows only. */
export function pickableAgentValues(sections: AgentListSection[]): string[] {
  const out: string[] = []
  for (const s of sections) for (const r of s.rows) if (r.state === 'pickable') out.push(r.value)
  return out
}

const searchInputClass =
  'w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]'

function rowClass(row: AgentListRow, isKeyboard: boolean): string {
  const base = 'flex items-center gap-2 px-2 py-1.5 transition-colors w-full text-left'
  if (row.state !== 'pickable') {
    return `${base} cursor-default ${row.state === 'disabled' ? 'opacity-50 ' : ''}text-[var(--color-text-muted)]`
  }
  return `flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left ${
    row.selected
      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
      : isKeyboard
        ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
        : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
  }`
}

export function SearchableAgentList({
  sections,
  query,
  onQuery,
  onPick,
  placeholder,
  emptyText,
  onEscape,
  matches,
  autoFocus = false,
  ariaLabel,
  listClassName = 'max-h-72 overflow-y-auto py-1',
}: SearchableAgentListProps): React.JSX.Element {
  const listId = useId()
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const searchRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  const shown = useMemo(() => visibleAgentSections(sections, query, matches), [sections, query, matches])
  const flatValues = useMemo(() => pickableAgentValues(shown), [shown])

  useEffect(() => {
    if (!autoFocus) return
    const raf = requestAnimationFrame(() => searchRef.current?.focus())
    return () => cancelAnimationFrame(raf)
  }, [autoFocus])

  useEffect(() => setKeyboardIndex(-1), [query])

  // Keep the keyboard-highlighted row in view.
  useEffect(() => {
    if (keyboardIndex < 0) return
    const val = flatValues[keyboardIndex]
    if (!val) return
    const el = listRef.current?.querySelector(`[data-ws-filter-value="${CSS.escape(val)}"]`)
    el?.scrollIntoView({ block: 'nearest' })
  }, [keyboardIndex, flatValues])

  const optionId = (value: string): string => `${listId}-opt-${flatValues.indexOf(value)}`
  const keyboardValue = keyboardIndex >= 0 ? flatValues[keyboardIndex] : undefined

  const onSearchKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.min(prev + 1, flatValues.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < flatValues.length) {
      e.preventDefault()
      onPick(flatValues[keyboardIndex])
    } else if (e.key === 'Escape' && onEscape) {
      e.preventDefault()
      e.stopPropagation()
      onEscape()
    }
  }

  return (
    <>
      <div className="p-1.5 border-b border-[var(--color-border)]">
        <input
          ref={searchRef}
          type="text"
          role="combobox"
          aria-expanded={true}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={keyboardValue !== undefined ? optionId(keyboardValue) : undefined}
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          onKeyDown={onSearchKeyDown}
          placeholder={placeholder}
          className={searchInputClass}
        />
      </div>

      <div ref={listRef} id={listId} role="listbox" aria-label={ariaLabel} className={listClassName}>
        {shown.map((section) => (
          <div key={section.key} role="group" aria-label={section.label ?? undefined} data-agent-list-section={section.key}>
            {section.label !== null && (
              <div className="flex items-center gap-1.5 px-2 pt-2 pb-1 select-none">
                {section.accent ? (
                  <span className="w-1 h-3 flex-shrink-0 bg-[var(--color-accent)]" />
                ) : (
                  section.color && (
                    <span className="w-1 h-3 flex-shrink-0" style={{ backgroundColor: section.color }} />
                  )
                )}
                <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] flex-1 truncate">
                  {section.label}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                  {section.count ?? section.rows.length}
                </span>
              </div>
            )}
            {section.status && <div data-agent-list-status={section.key}>{section.status}</div>}
            {section.rows.map((row) => {
              const pickable = row.state === 'pickable'
              const isKeyboard = pickable && row.value === keyboardValue
              return (
                <button
                  key={row.value}
                  type="button"
                  role="option"
                  id={pickable ? optionId(row.value) : undefined}
                  aria-selected={row.selected === true || row.state === 'checked'}
                  aria-disabled={pickable ? undefined : true}
                  data-ws-filter-value={row.value}
                  data-row-state={row.state}
                  onClick={() => {
                    if (pickable) onPick(row.value)
                  }}
                  className={rowClass(row, isKeyboard)}
                  title={row.title}
                >
                  {row.avatar.kind === 'workspace' ? (
                    <ProjectAvatar
                      projectPath={row.avatar.path}
                      projectName={row.avatar.name}
                      projectColor={row.avatar.color}
                      projectId={row.avatar.id}
                      iconUrl={row.avatar.iconUrl}
                      size={20}
                      fetchIcon={row.avatar.fetchIcon}
                    />
                  ) : (
                    row.avatar.node
                  )}
                  <span className={row.labelClassName ? `text-xs truncate flex-1 ${row.labelClassName}` : 'text-xs truncate flex-1'}>
                    {row.label}
                  </span>
                  {row.detail !== undefined && (
                    <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                      {row.detail}
                    </span>
                  )}
                  {row.state === 'checked' && (
                    <svg
                      className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]"
                      fill="none"
                      viewBox="0 0 24 24"
                      stroke="currentColor"
                      strokeWidth={2.5}
                      aria-hidden
                      data-row-check=""
                    >
                      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                    </svg>
                  )}
                </button>
              )
            })}
          </div>
        ))}

        {shown.length === 0 && (
          <div className="px-2 py-4 text-center text-[10px] text-[var(--color-text-muted)]">{emptyText}</div>
        )}
      </div>
    </>
  )
}
