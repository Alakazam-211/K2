// The Wiki header's workspace picker (0.46.0, Rosson 2026-10-16): which
// wiki the page shows. The agent pickers' look — the nav's focus-group
// trigger (`Sidebar/FocusGroupDropdown`: bordered button, uppercase name via
// `PickerTriggerLabel`, chevron) with the workspace's avatar, and the shared
// `SearchableAgentList` body (search box, focus-group sections with their
// colour bars, avatar rows) that Tickets' filter and Home's Add Agent use.
//
// Rows are this window's server's workspaces. The first row is "All
// workspaces": the server-wide map (the page's `k2` scope). Picking a row
// only changes the wiki shown; the room you came from is untouched.
//
// The menu is portalled (`useAnchoredMenu`). Esc closes it in the capture
// phase, before the page's own Esc (clear the note / close the wiki).

import React, { useEffect, useMemo, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { PickerTriggerLabel } from '@/components/Sidebar/picker-label'
import { SearchableAgentList, type AgentListRow, type AgentListSection } from '@/components/ui/SearchableAgentList'
import {
  AllWorkspacesGlyph,
  groupWorkspacesForFilter,
  type FilterableWorkspace,
} from '@/components/Feedback/WorkspaceFilterDropdown'

/** The picker's value for the server-wide map. Workspace values are paths
 *  (absolute), so it cannot collide. */
export const WIKI_ALL_WORKSPACES = 'all'
export const WIKI_ALL_WORKSPACES_LABEL = 'All workspaces'

export interface WikiPickerWorkspace extends FilterableWorkspace {
  /** Worktree paths under this workspace (the nav's View Wiki opens those). */
  worktreePaths?: readonly string[]
}

/** The workspace a wiki path belongs to: its own path, or one of its
 *  worktrees. */
export function wikiWorkspaceForPath<W extends WikiPickerWorkspace>(workspaces: readonly W[], path: string | null): W | null {
  if (!path) return null
  return (
    workspaces.find((w) => w.path === path) ??
    workspaces.find((w) => (w.worktreePaths ?? []).includes(path)) ??
    null
  )
}

/** The menu's sections (pure): All workspaces on top (no query), then this
 *  server's workspaces grouped by focus group (alphabetical within). */
export function wikiPickerSections(
  workspaces: readonly WikiPickerWorkspace[],
  focusGroups: Parameters<typeof groupWorkspacesForFilter>[1],
  focusGroupsEnabled: boolean,
  query: string,
  value: string,
): AgentListSection[] {
  const out: AgentListSection[] = []
  if (!query.trim()) {
    out.push({
      key: '__all__',
      label: null,
      rows: [
        {
          value: WIKI_ALL_WORKSPACES,
          label: WIKI_ALL_WORKSPACES_LABEL,
          avatar: { kind: 'node', node: <AllWorkspacesGlyph /> },
          state: 'pickable',
          selected: value === WIKI_ALL_WORKSPACES,
          title: 'Every workspace brain on this server, one map',
        },
      ],
    })
  }
  const selectedWs = value === WIKI_ALL_WORKSPACES ? null : wikiWorkspaceForPath(workspaces, value)
  for (const section of groupWorkspacesForFilter([...workspaces], focusGroups, focusGroupsEnabled, query)) {
    out.push({
      key: section.key,
      label: section.label,
      color: section.color,
      rows: section.workspaces.map(
        (ws): AgentListRow => ({
          value: ws.path,
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
          selected: selectedWs?.id === ws.id,
        }),
      ),
    })
  }
  return out
}

export function WikiWorkspacePicker({
  workspaces,
  value,
  onChange,
}: {
  workspaces: readonly WikiPickerWorkspace[]
  /** `WIKI_ALL_WORKSPACES`, or the open wiki's workspace path. */
  value: string | null
  onChange: (value: string) => void
}): React.JSX.Element {
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const menu = useAnchoredMenu<HTMLDivElement>({ open, onClose: () => setOpen(false), width: 'min', minWidth: 256 })

  const isAll = value === WIKI_ALL_WORKSPACES
  const selected = isAll ? null : wikiWorkspaceForPath(workspaces, value)
  const triggerName = isAll
    ? WIKI_ALL_WORKSPACES_LABEL
    : selected?.name ?? (value ? value.split('/').filter(Boolean).pop() ?? value : 'No workspace')

  const sections = useMemo(
    () => wikiPickerSections(workspaces, focusGroups, focusGroupsEnabled, query, value ?? ''),
    [workspaces, focusGroups, focusGroupsEnabled, query, value],
  )

  useEffect(() => {
    if (!open) setQuery('')
  }, [open])

  // Capture-phase Esc: close the menu before the page's Esc handler.
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      setOpen(false)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open])

  const pick = (v: string): void => {
    setOpen(false)
    if (v !== value) onChange(v)
  }

  return (
    <div ref={menu.anchorRef} className="relative no-drag w-56 flex-shrink-0" data-wiki-workspace-picker="">
      <button
        type="button"
        data-wiki-picker-trigger=""
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Wiki workspace: ${triggerName}`}
        title="Pick which workspace's wiki to show"
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-2 px-2 py-1 text-left bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer"
      >
        {isAll ? (
          <AllWorkspacesGlyph />
        ) : selected ? (
          <ProjectAvatar
            projectPath={selected.path}
            projectName={selected.name}
            projectColor={selected.color}
            projectId={selected.id}
            iconUrl={selected.iconUrl}
            size={20}
          />
        ) : null}
        <PickerTriggerLabel name={triggerName} />
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open &&
        menu.portal(
          <div
            ref={menu.menuRef}
            style={menu.style}
            data-wiki-picker-menu=""
            data-placement={menu.placement}
            className="no-drag bg-[var(--color-bg)] border border-[var(--color-border)] shadow-xl flex flex-col"
          >
            <SearchableAgentList
              sections={sections}
              query={query}
              onQuery={setQuery}
              onPick={pick}
              onEscape={() => setOpen(false)}
              placeholder="Search workspaces..."
              emptyText="No workspaces match"
              ariaLabel="Wiki workspaces"
              autoFocus
            />
          </div>,
        )}
    </div>
  )
}
