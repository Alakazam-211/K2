// Feedback board — custom workspace-filter dropdown.
//
// Mirrors the Settings → Workspaces list's ergonomics inside a popover
// (ProjectsSection's left panel): a search input at the top filtering
// the rows live, focus-group grouping when that feature is on (group
// header = color bar + name + count), alphabetical within groups (and
// alphabetical flat when groups are off), each row carrying the
// workspace icon with its colored border via ProjectAvatar. An "All
// workspaces" row sits on top. Square corners, dark, K2 tokens.
//
// Data comes from the SAME stores the settings page reads
// (useProjectsStore via the parent's `projects` prop +
// useFocusGroupsStore for groups/enabled) — no duplicated plumbing.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useFocusGroupsStore, type FocusGroup } from '@/stores/focus-groups'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'

/** The slice of a project row the dropdown needs (a subset of the
 *  projects store's shape, so the page can pass its rows straight in). */
export interface FilterableWorkspace {
  id: string
  name: string
  path: string
  color: string
  iconUrl: string | null
  focusGroupId: string | null
}

export interface WorkspaceFilterSection {
  /** Stable key for React; group id, '__ungrouped__', or '__flat__'. */
  key: string
  /** Header label; null = the flat (groups-off) section, no header. */
  label: string | null
  /** The focus group's color bar, when it has one. */
  color: string | null
  workspaces: FilterableWorkspace[]
}

/** Same filter the settings page's workspace search applies: substring
 *  on name OR path, case-insensitive; empty query matches everything. */
export function workspaceMatchesSearch(ws: { name: string; path: string }, query: string): boolean {
  if (!query.trim()) return true
  const q = query.toLowerCase()
  return ws.name.toLowerCase().includes(q) || ws.path.toLowerCase().includes(q)
}

/** Pure section builder (unit-tested): focus-group grouping when
 *  enabled (groups in store/tab order, then Ungrouped), alphabetical
 *  within every section, empty sections dropped; groups off = one
 *  flat alphabetical section. */
export function groupWorkspacesForFilter(
  projects: FilterableWorkspace[],
  focusGroups: FocusGroup[],
  focusGroupsEnabled: boolean,
  query: string,
): WorkspaceFilterSection[] {
  const byName = (a: FilterableWorkspace, b: FilterableWorkspace): number =>
    a.name.localeCompare(b.name)
  const matching = projects.filter((p) => workspaceMatchesSearch(p, query))

  if (!focusGroupsEnabled) {
    const all = [...matching].sort(byName)
    return all.length > 0 ? [{ key: '__flat__', label: null, color: null, workspaces: all }] : []
  }

  const sections: WorkspaceFilterSection[] = []
  for (const group of focusGroups) {
    const ws = matching.filter((p) => p.focusGroupId === group.id).sort(byName)
    if (ws.length > 0) {
      sections.push({ key: group.id, label: group.name, color: group.color, workspaces: ws })
    }
  }
  const ungrouped = matching.filter((p) => !p.focusGroupId).sort(byName)
  if (ungrouped.length > 0) {
    sections.push({ key: '__ungrouped__', label: 'Ungrouped', color: null, workspaces: ungrouped })
  }
  return sections
}

interface WorkspaceFilterDropdownProps {
  projects: FilterableWorkspace[]
  /** 'all' or a project id. */
  value: string
  onChange: (value: string) => void
}

export function WorkspaceFilterDropdown({
  projects,
  value,
  onChange,
}: WorkspaceFilterDropdownProps): React.JSX.Element {
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)

  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const rootRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  const sections = useMemo(
    () => groupWorkspacesForFilter(projects, focusGroups, focusGroupsEnabled, query),
    [projects, focusGroups, focusGroupsEnabled, query],
  )

  // Flattened selectable values for ArrowUp/Down + Enter (the settings
  // search's keyboard feel). The All row only shows without a query.
  const showAllRow = !query.trim()
  const flatValues = useMemo(() => {
    const vals: string[] = showAllRow ? ['all'] : []
    for (const s of sections) for (const ws of s.workspaces) vals.push(ws.id)
    return vals
  }, [sections, showAllRow])

  const selected = value === 'all' ? null : projects.find((p) => p.id === value) ?? null

  // Focus the search on open; reset transient state on close.
  useEffect(() => {
    if (open) {
      requestAnimationFrame(() => searchRef.current?.focus())
    } else {
      setQuery('')
      setKeyboardIndex(-1)
    }
  }, [open])

  useEffect(() => setKeyboardIndex(-1), [query])

  // Outside click closes; capture-phase Esc closes the popover BEFORE
  // the page-level Esc handler (clear selection / close page) sees it.
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open])

  // Keep the keyboard-highlighted row in view.
  useEffect(() => {
    if (keyboardIndex < 0) return
    const val = flatValues[keyboardIndex]
    if (!val) return
    const el = rootRef.current?.querySelector(`[data-ws-filter-value="${CSS.escape(val)}"]`)
    el?.scrollIntoView({ block: 'nearest' })
  }, [keyboardIndex, flatValues])

  const pick = (v: string): void => {
    onChange(v)
    setOpen(false)
  }

  const onSearchKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.min(prev + 1, flatValues.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < flatValues.length) {
      e.preventDefault()
      pick(flatValues[keyboardIndex])
    } else if (e.key === 'Escape') {
      // Stop the page-level Esc (clear selection / close page) — the
      // first Esc only closes this popover.
      e.preventDefault()
      e.stopPropagation()
      setOpen(false)
    }
  }

  const rowClass = (isSelected: boolean, isKeyboard: boolean): string =>
    `flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left ${
      isSelected
        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
        : isKeyboard
          ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
    }`

  return (
    <div ref={rootRef} className="relative flex-shrink-0">
      {/* Trigger — sized like the old <select>, showing the current pick. */}
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 px-2 py-1.5 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] outline-none cursor-pointer hover:text-[var(--color-text-primary)] transition-colors max-w-[180px]"
        title="Filter by workspace"
      >
        {selected && (
          <ProjectAvatar
            projectPath={selected.path}
            projectName={selected.name}
            projectColor={selected.color}
            projectId={selected.id}
            iconUrl={selected.iconUrl}
            size={16}
          />
        )}
        <span className="truncate">{selected ? selected.name : 'All workspaces'}</span>
        <svg
          className={`w-2.5 h-2.5 flex-shrink-0 text-[var(--color-text-muted)] transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 w-64 z-30 bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-lg flex flex-col">
          {/* Search — same feel as the settings page's workspace search. */}
          <div className="p-1.5 border-b border-[var(--color-border)]">
            <input
              ref={searchRef}
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onSearchKeyDown}
              placeholder="Search workspaces..."
              className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>

          <div className="max-h-72 overflow-y-auto py-1">
            {showAllRow && (
              <button
                type="button"
                data-ws-filter-value="all"
                onClick={() => pick('all')}
                className={rowClass(value === 'all', keyboardIndex === 0)}
              >
                {/* Stand-in glyph so the label aligns with avatar rows. */}
                <span className="flex-shrink-0 w-5 h-5 flex items-center justify-center border border-[var(--color-border)] text-[var(--color-text-muted)]">
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <rect x="3" y="3" width="7" height="7" />
                    <rect x="14" y="3" width="7" height="7" />
                    <rect x="3" y="14" width="7" height="7" />
                    <rect x="14" y="14" width="7" height="7" />
                  </svg>
                </span>
                <span className="text-xs truncate flex-1">All workspaces</span>
              </button>
            )}

            {sections.map((section) => (
              <div key={section.key}>
                {section.label !== null && (
                  <div className="flex items-center gap-1.5 px-2 pt-2 pb-1 select-none">
                    {section.color && (
                      <span className="w-1 h-3 flex-shrink-0" style={{ backgroundColor: section.color }} />
                    )}
                    <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] flex-1 truncate">
                      {section.label}
                    </span>
                    <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                      {section.workspaces.length}
                    </span>
                  </div>
                )}
                {section.workspaces.map((ws) => {
                  const kbIdx = flatValues.indexOf(ws.id)
                  return (
                    <button
                      key={ws.id}
                      type="button"
                      data-ws-filter-value={ws.id}
                      onClick={() => pick(ws.id)}
                      className={rowClass(value === ws.id, kbIdx >= 0 && kbIdx === keyboardIndex)}
                    >
                      <ProjectAvatar
                        projectPath={ws.path}
                        projectName={ws.name}
                        projectColor={ws.color}
                        projectId={ws.id}
                        iconUrl={ws.iconUrl}
                        size={20}
                      />
                      <span className="text-xs truncate flex-1">{ws.name}</span>
                    </button>
                  )
                })}
              </div>
            ))}

            {sections.length === 0 && (
              <div className="px-2 py-4 text-center text-[10px] text-[var(--color-text-muted)]">
                No workspaces match
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
