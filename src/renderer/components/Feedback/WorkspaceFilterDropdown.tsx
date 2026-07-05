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
// Projects V1 P7 (prd-projects-v1 §6.6): a visually distinct "Projects"
// section sits between the All row and the workspace sections — picking
// a project filters the board to its MEMBER workspaces (a pure
// client-side filter; the page resolves membership via
// /cli/project-group/show). Project values ride the same string-value
// channel as workspace ids under a `project:` prefix.
//
// Data comes from the SAME stores the settings page reads
// (useProjectsStore via the parent's `projects` prop +
// useFocusGroupsStore for groups/enabled + useProjectGroupsStore for
// the project-group rows) — no duplicated plumbing.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useFocusGroupsStore, type FocusGroup } from '@/stores/focus-groups'
import { useProjectGroupsStore } from '@/stores/project-groups'
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

// ── Projects V1 P7 (§6.6) — project-group filter values ───────────────────

/** The slice of a project-group row the dropdown needs (a subset of the
 *  project-groups store's shape, passed straight in). */
export interface FilterableProjectGroup {
  id: string
  name: string
  memberCount: number
}

/** Project filter values ride the same string channel as workspace ids;
 *  the prefix keeps them unambiguous (workspace ids never contain it). */
const PROJECT_FILTER_PREFIX = 'project:'

export function projectFilterValue(groupId: string): string {
  return `${PROJECT_FILTER_PREFIX}${groupId}`
}

/** The group id when `value` is a project filter, else null. */
export function parseProjectFilter(value: string): string | null {
  return value.startsWith(PROJECT_FILTER_PREFIX)
    ? value.slice(PROJECT_FILTER_PREFIX.length)
    : null
}

/** Pure section builder for the Projects section (unit-tested):
 *  substring match on the name, alphabetical (the dropdown's workspace
 *  ordering idiom — the nav's pinned-first order is a nav concern). */
export function filterProjectGroupsForFilter(
  groups: FilterableProjectGroup[],
  query: string,
): FilterableProjectGroup[] {
  const q = query.trim().toLowerCase()
  return groups
    .filter((g) => !q || g.name.toLowerCase().includes(q))
    .sort((a, b) => a.name.localeCompare(b.name))
}

/** The board's client-side workspace filter (unit-tested): 'all' passes
 *  everything; a `project:` value keeps rows whose host workspace is a
 *  MEMBER of that project (null membership = still resolving → empty);
 *  anything else is a single workspace id. */
export function rowsForWorkspaceFilter<T extends { projectId: string }>(
  rows: T[],
  value: string,
  projectMemberIds: ReadonlySet<string> | null,
): T[] {
  if (value === 'all') return rows
  if (parseProjectFilter(value) !== null) {
    return rows.filter((r) => projectMemberIds?.has(r.projectId) ?? false)
  }
  return rows.filter((r) => r.projectId === value)
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
  /** 'all', a workspace id, or `project:<groupId>` (§6.6). */
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
  // Project-group rows for the Projects section (null until the boot
  // fetch lands — render as empty, the badge wiring keeps it fresh).
  const projectGroups = useProjectGroupsStore((s) => s.groups)

  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const rootRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  const sections = useMemo(
    () => groupWorkspacesForFilter(projects, focusGroups, focusGroupsEnabled, query),
    [projects, focusGroups, focusGroupsEnabled, query],
  )

  const groupRows = useMemo(
    () => filterProjectGroupsForFilter(projectGroups ?? [], query),
    [projectGroups, query],
  )

  // Flattened selectable values for ArrowUp/Down + Enter (the settings
  // search's keyboard feel). The All row only shows without a query.
  const showAllRow = !query.trim()
  const flatValues = useMemo(() => {
    const vals: string[] = showAllRow ? ['all'] : []
    for (const g of groupRows) vals.push(projectFilterValue(g.id))
    for (const s of sections) for (const ws of s.workspaces) vals.push(ws.id)
    return vals
  }, [sections, groupRows, showAllRow])

  const selectedGroupId = parseProjectFilter(value)
  const selectedGroup =
    selectedGroupId !== null
      ? (projectGroups ?? []).find((g) => g.id === selectedGroupId) ?? null
      : null
  const selected =
    value === 'all' || selectedGroupId !== null
      ? null
      : projects.find((p) => p.id === value) ?? null

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
        title="Filter by workspace or project"
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
        {selectedGroup && <ProjectGroupGlyph size={16} />}
        <span className="truncate">
          {selected ? selected.name : selectedGroup ? selectedGroup.name : 'All workspaces'}
        </span>
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

            {/* Projects section (§6.6) — visually distinct: accent
                header + group glyph rows; picking one filters the board
                to that project's member workspaces. */}
            {groupRows.length > 0 && (
              <div>
                <div className="flex items-center gap-1.5 px-2 pt-2 pb-1 select-none">
                  <span className="w-1 h-3 flex-shrink-0 bg-[var(--color-accent)]" />
                  <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] flex-1 truncate">
                    Projects
                  </span>
                  <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                    {groupRows.length}
                  </span>
                </div>
                {groupRows.map((g) => {
                  const gv = projectFilterValue(g.id)
                  const kbIdx = flatValues.indexOf(gv)
                  return (
                    <button
                      key={g.id}
                      type="button"
                      data-ws-filter-value={gv}
                      onClick={() => pick(gv)}
                      className={rowClass(value === gv, kbIdx >= 0 && kbIdx === keyboardIndex)}
                      title={`Only feedback from ${g.name}'s member workspaces`}
                    >
                      <ProjectGroupGlyph size={20} />
                      <span className="text-xs truncate flex-1">{g.name}</span>
                      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                        {g.memberCount}
                      </span>
                    </button>
                  )
                })}
              </div>
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

            {sections.length === 0 && groupRows.length === 0 && (
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

/** Stand-in glyph for project rows (stacked layers — distinct from the
 *  workspace avatars), sized to align with ProjectAvatar rows. */
function ProjectGroupGlyph({ size }: { size: number }): React.JSX.Element {
  return (
    <span
      className="flex-shrink-0 flex items-center justify-center border border-[var(--color-accent)]/40 text-[var(--color-accent)]"
      style={{ width: size, height: size }}
    >
      <svg width={size - 8} height={size - 8} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <polygon points="12 2 2 7 12 12 22 7 12 2" />
        <polyline points="2 17 12 22 22 17" />
        <polyline points="2 12 12 17 22 12" />
      </svg>
    </span>
  )
}
