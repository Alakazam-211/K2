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
// the project-group rows) — no duplicated plumbing. The popover body
// (search, sections, rows, keyboard) is the shared `SearchableAgentList`,
// which Home's Add Agent picker uses too; this wrapper keeps the trigger,
// the popover, its close rules and the store reads.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useFocusGroupsStore, type FocusGroup } from '@/stores/focus-groups'
import { useProjectGroupsStore } from '@/stores/project-groups'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import {
  SearchableAgentList,
  type AgentListRow,
  type AgentListSection,
} from '@/components/ui/SearchableAgentList'

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

/** Filter value for tickets whose workspace was removed. Workspace ids are
 *  UUIDs and project filters carry `project:`, so it cannot collide. */
export const UNLINKED_FILTER_VALUE = 'unlinked'
export const UNLINKED_FILTER_LABEL = 'Unlinked workspace'

/** The board's client-side workspace filter (unit-tested): 'all' passes
 *  everything; `unlinked` keeps rows whose workspace is gone; a `project:`
 *  value keeps rows whose host workspace is a MEMBER of that project (null
 *  membership = still resolving → empty); anything else is a single
 *  workspace id (an unlinked row never matches one). */
export function rowsForWorkspaceFilter<T extends { projectId: string; linked?: boolean }>(
  rows: T[],
  value: string,
  projectMemberIds: ReadonlySet<string> | null,
): T[] {
  if (value === 'all') return rows
  if (value === UNLINKED_FILTER_VALUE) return rows.filter((r) => r.linked === false)
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
  /** 'all', a workspace id, `project:<groupId>` (§6.6), or `unlinked`. */
  value: string
  onChange: (value: string) => void
  /** Offer the "Unlinked workspace" row (only while such tickets exist). */
  showUnlinked?: boolean
}

export function WorkspaceFilterDropdown({
  projects,
  value,
  onChange,
  showUnlinked = false,
}: WorkspaceFilterDropdownProps): React.JSX.Element {
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  // Project-group rows for the Projects section (null until the boot
  // fetch lands — render as empty, the badge wiring keeps it fresh).
  const projectGroups = useProjectGroupsStore((s) => s.groups)

  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)

  const sections = useMemo(
    () => groupWorkspacesForFilter(projects, focusGroups, focusGroupsEnabled, query),
    [projects, focusGroups, focusGroupsEnabled, query],
  )

  const groupRows = useMemo(
    () => filterProjectGroupsForFilter(projectGroups ?? [], query),
    [projectGroups, query],
  )

  // The All row only shows without a query.
  const showAllRow = !query.trim()
  const showUnlinkedRow = showUnlinked && workspaceMatchesSearch({ name: UNLINKED_FILTER_LABEL, path: '' }, query)

  const selectedGroupId = parseProjectFilter(value)
  const selectedGroup =
    selectedGroupId !== null
      ? (projectGroups ?? []).find((g) => g.id === selectedGroupId) ?? null
      : null
  const unlinkedSelected = value === UNLINKED_FILTER_VALUE
  const selected =
    value === 'all' || selectedGroupId !== null || unlinkedSelected
      ? null
      : projects.find((p) => p.id === value) ?? null

  // The shared list's sections, in the board's order: All + Unlinked (no
  // header), Projects (accent header), then the workspace sections.
  const listSections = useMemo(
    () =>
      workspaceFilterListSections({
        showAllRow,
        showUnlinkedRow,
        groupRows,
        sections,
        value,
      }),
    [showAllRow, showUnlinkedRow, groupRows, sections, value],
  )

  // Reset transient state on close.
  useEffect(() => {
    if (!open) setQuery('')
  }, [open])

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

  const pick = (v: string): void => {
    onChange(v)
    setOpen(false)
  }

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
          {selected
            ? selected.name
            : selectedGroup
              ? selectedGroup.name
              : unlinkedSelected
                ? UNLINKED_FILTER_LABEL
                : 'All workspaces'}
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
        <div className="absolute right-0 top-full mt-1 w-64 z-30 bg-[var(--color-bg)] border border-[var(--color-border)] shadow-lg flex flex-col">
          {/* Search + rows: the shared list (search, sections, keyboard). */}
          <SearchableAgentList
            sections={listSections}
            query={query}
            onQuery={setQuery}
            onPick={pick}
            onEscape={() => setOpen(false)}
            placeholder="Search workspaces..."
            emptyText="No workspaces match"
            ariaLabel="Workspaces"
            autoFocus
          />
        </div>
      )}
    </div>
  )
}

/** The dropdown's rows as shared-list sections (already filtered for the
 *  query by the pure helpers above). */
function workspaceFilterListSections({
  showAllRow,
  showUnlinkedRow,
  groupRows,
  sections,
  value,
}: {
  showAllRow: boolean
  showUnlinkedRow: boolean
  groupRows: FilterableProjectGroup[]
  sections: WorkspaceFilterSection[]
  value: string
}): AgentListSection[] {
  const out: AgentListSection[] = []
  const loose: AgentListRow[] = []
  if (showAllRow) {
    loose.push({
      value: 'all',
      label: 'All workspaces',
      avatar: { kind: 'node', node: <AllWorkspacesGlyph /> },
      state: 'pickable',
      selected: value === 'all',
    })
  }
  if (showUnlinkedRow) {
    loose.push({
      value: UNLINKED_FILTER_VALUE,
      label: UNLINKED_FILTER_LABEL,
      labelClassName: 'italic',
      avatar: { kind: 'node', node: <UnlinkedGlyph /> },
      state: 'pickable',
      selected: value === UNLINKED_FILTER_VALUE,
      title: 'Tickets whose workspace was removed from this server',
    })
  }
  if (loose.length > 0) out.push({ key: '__loose__', label: null, rows: loose })
  // Projects section (§6.6) — visually distinct: accent header + group
  // glyph rows; picking one filters the board to that project's member
  // workspaces.
  if (groupRows.length > 0) {
    out.push({
      key: '__projects__',
      label: 'Projects',
      accent: true,
      rows: groupRows.map((g): AgentListRow => {
        const gv = projectFilterValue(g.id)
        return {
          value: gv,
          label: g.name,
          detail: g.memberCount,
          avatar: { kind: 'node', node: <ProjectGroupGlyph size={20} /> },
          state: 'pickable',
          selected: value === gv,
          title: `Only feedback from ${g.name}'s member workspaces`,
        }
      }),
    })
  }
  for (const section of sections) {
    out.push({
      key: section.key,
      label: section.label,
      color: section.color,
      rows: section.workspaces.map(
        (ws): AgentListRow => ({
          value: ws.id,
          label: ws.name,
          avatar: {
            kind: 'workspace',
            path: ws.path,
            name: ws.name,
            color: ws.color,
            id: ws.id,
            iconUrl: ws.iconUrl,
            fetchIcon: true,
          },
          state: 'pickable',
          selected: value === ws.id,
        }),
      ),
    })
  }
  return out
}

/** Stand-in glyph for the All row, so its label aligns with avatar rows. */
function AllWorkspacesGlyph(): React.JSX.Element {
  return (
    <span className="flex-shrink-0 w-5 h-5 flex items-center justify-center border border-[var(--color-border)] text-[var(--color-text-muted)]">
      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
        <rect x="3" y="3" width="7" height="7" />
        <rect x="14" y="3" width="7" height="7" />
        <rect x="3" y="14" width="7" height="7" />
        <rect x="14" y="14" width="7" height="7" />
      </svg>
    </span>
  )
}

/** The Unlinked row's dashed "?" box. */
function UnlinkedGlyph(): React.JSX.Element {
  return (
    <span className="flex-shrink-0 w-5 h-5 flex items-center justify-center border border-dashed border-[var(--color-border)] text-[var(--color-text-muted)]">
      ?
    </span>
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
