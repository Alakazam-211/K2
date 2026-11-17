// Projects V1 P4 (prd-projects-v1 §6.1) — the Projects page's LEFT NAV,
// in two parts (0.46.12, Rosson 2026-11-11):
//   1. which project: a "Projects" header with "+" (New project), then
//      Pinned and All projects as Settings → Projects shows them (sentence
//      case, "6 agents"). The list has a capped height and scrolls on its
//      own, so it never pushes the rest out of a small window;
//   2. a divider, then the SELECTED project: its picture and name, its
//      agents (each with its files folded under it) and "Add an agent…"
//      (ProjectAgentsSection.tsx), scrolling on its own too.
// The nav sits in the Home and Agents navs' panel (NavTile, set by the
// page); its top row is level with the project bar beside it (the nav's top
// row matches the tabs beside it, as on Home and Agents). It
// collapses to an icon rail (a tile of its own: projects on top, the
// selected project's agents as pictures below); « sits at the right end of
// the top row and » at the top of the rail, Home's places. Group-list
// drag-to-reorder awaits a sort-order route (V1.1).

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { openProjectSettings } from './ProjectSettingsSheet'
import { NEW_PROJECT_EVENT } from './project-nav-events'
import { useToastStore } from '@/stores/toast'
import { showContextMenu } from '@/lib/context-menu'
import { DoubleChevron } from '@/components/Sidebar/SidebarCollapseButton'
import { NAV_TOGGLE_BOX } from '@/components/Layout/drawer-toggles'
import { DrawerHeaderButton } from '@/components/Layout/DrawerTabBarToggle'
import { PROJECT_BAR_ROW_HEIGHT } from './project-bar-height'
import { Button, Input, cx } from '@/components/ui'
import {
  AVATAR_ROUND,
  SETTING_LIST_ROW_HOVER,
} from '@/components/Settings/controls/SettingControls'
import ProjectGroupAvatar from './ProjectGroupAvatar'
import { agentCountText } from './project-settings'
import { ProjectNavSection, RailAgents } from './ProjectAgentsSection'
import {
  createProjectGroup,
  createErrorMessage,
  partitionPinned,
  pinProjectGroup,
  type ProjectGroup,
  type ProjectGroupDashboard,
  type ProjectGroupShow,
} from './projects-api'
import { primaryScope } from '@/kessel/server-scope'
import { ConversationUnreadBadge, muteMenuState, toggleMute } from '@/components/common/UnreadBadge'
import { unreadKey, unreadSupported, useUnreadStore } from '@/stores/unread'

export const PROJECT_NAV_WIDTH = 296
/** Collapsed-rail width (IconRail's RAIL_WIDTH). */
export const PROJECT_NAV_RAIL_WIDTH = 48


// ── Group row (the Settings → Projects list row) ──────────────────────────

/** The project row menu's mute item (thread-read-v1), or null on a server
 *  without it. Reads the store at click time, never in render. */
function groupMuteItem(groupId: string): { label: string; next: boolean } | null {
  if (!unreadSupported()) return null
  const muted = useUnreadStore.getState().byKey[unreadKey('project', groupId)]?.muted === true
  return muteMenuState(muted, 'chat')
}

/** The one right-click menu for a project, in the nav row and the
 *  collapsed rail alike: Project settings (a sheet over this page with
 *  this project's settings; `openProjectSettings`), Pin to top / Unpin,
 *  and Mute when the server has it. `togglePin` is the caller's pin
 *  action; `opener` (the row) gets the focus back when the sheet closes. */
export async function openProjectGroupContextMenu(
  group: ProjectGroup,
  togglePin: () => Promise<void>,
  opener?: HTMLElement | null,
): Promise<void> {
  const mute = groupMuteItem(group.id)
  const clickedId = await showContextMenu([
    { id: 'group-settings', label: 'Project settings' },
    { id: 'separator-pin', label: '', type: 'separator' },
    { id: 'toggle-pin', label: group.pinned ? 'Unpin' : 'Pin to top' },
    ...(mute ? [{ id: 'toggle-mute', label: mute.label }] : []),
  ])
  if (clickedId === 'toggle-mute' && mute) {
    await toggleMute('project', group.id, mute.next, 'project chat')
  } else if (clickedId === 'group-settings') {
    openProjectSettings(group.id, opener)
  } else if (clickedId === 'toggle-pin') {
    await togglePin()
  }
}

/** Pin or unpin a project, refetch the list so it moves sections, and say
 *  so when the server refuses. */
async function setGroupPinned(group: ProjectGroup): Promise<void> {
  try {
    await pinProjectGroup(primaryScope(), group.id, !group.pinned)
    // groups-changed also fires; refetch now so the row moves sections
    // without waiting out the coalesce window.
    await useProjectGroupsStore.getState().fetchGroups()
  } catch (err) {
    useToastStore
      .getState()
      .addToast(
        `Couldn’t ${group.pinned ? 'unpin' : 'pin'} ${group.name}: ${err instanceof Error ? err.message : String(err)}`,
        'error',
      )
  }
}

function GroupRow({
  group,
  selected,
  onSelect,
}: {
  group: ProjectGroup
  selected: boolean
  onSelect: () => void
}): React.JSX.Element {
  const [pinBusy, setPinBusy] = useState(false)

  // Pin / Unpin lives in the right-click menu (a keyboard-reachable
  // menu, not a hover-only glyph); the Pinned section shows the state.
  const togglePin = async (): Promise<void> => {
    if (pinBusy) return
    setPinBusy(true)
    try {
      await setGroupPinned(group)
    } finally {
      setPinBusy(false)
    }
  }

  // Right-click menu (§6.5) — the Sidebar workspace-row idiom
  // (showContextMenu): "Project settings" deep-links to Settings →
  // Projects with THIS project preselected.
  const handleContextMenu = async (e: React.MouseEvent): Promise<void> => {
    e.preventDefault()
    await openProjectGroupContextMenu(group, () => togglePin(), e.currentTarget as HTMLElement)
  }

  return (
    <div className="no-drag" data-group-id={group.id}>
      <button
        type="button"
        title={`${group.name} · ${agentCountText(group.memberCount)}`}
        onContextMenu={(e) => void handleContextMenu(e)}
        onClick={onSelect}
        aria-current={selected ? 'true' : undefined}
        data-k2-focus=""
        className={cx(
          'relative w-full flex items-center gap-2.5 px-3 py-1.5 text-left outline-none cursor-pointer select-none',
          pinBusy ? 'opacity-60' : '',
          selected ? 'bg-[var(--color-accent)]/15' : SETTING_LIST_ROW_HOVER,
        )}
      >
        {selected && (
          <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />
        )}
        {/* §6.7.1/§6.7.7 — icon when set, else initials on the group's
            color (the shared avatar the rail and the bar draw too). */}
        <span className={AVATAR_ROUND}>
          <ProjectGroupAvatar name={group.name} groupId={group.id} size={20} color={group.color} />
        </span>
        <span className="min-w-0 flex-1 type-name text-[var(--color-text-primary)] truncate">{group.name}</span>
        <ConversationUnreadBadge kind="project" id={group.id} variant="dot" label="project messages" />
      </button>
    </div>
  )
}

// ── "+ New project" (inline create, surfaces name_taken — §6.1) ──────────

/** Inline name field + Create. POSTs `project-group/create`, surfaces
 *  `name_taken` inline, refetches + selects the new group on success.
 *  Shared by the nav's "+" and the page's no-projects empty state. */
export function CreateProjectForm({ onDone }: { onDone: () => void }): React.JSX.Element {
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (): Promise<void> => {
    const trimmed = name.trim()
    if (!trimmed || busy) return
    setBusy(true)
    try {
      const group = await createProjectGroup(primaryScope(), trimmed)
      const store = useProjectGroupsStore.getState()
      await store.fetchGroups()
      store.selectGroup(group.id)
      useToastStore.getState().addToast(`Created ${group.name ?? trimmed}.`, 'success')
      onDone()
    } catch (err) {
      setError(createErrorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-2" data-create-project-form="">
      <Input
        autoFocus
        type="text"
        aria-label="Project name"
        value={name}
        disabled={busy}
        aria-invalid={error ? true : undefined}
        onChange={(e) => {
          setName(e.target.value)
          setError(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void submit()
          // stopPropagation so the page-level Esc doesn't also fire —
          // Esc here just cancels the field.
          if (e.key === 'Escape') {
            e.stopPropagation()
            onDone()
          }
        }}
        placeholder="Project name"
        className="w-full"
      />
      {error && (
        <p role="alert" className="type-hint text-[var(--color-status-error-text)] leading-snug">
          {error}
        </p>
      )}
      <div className="flex items-center justify-end gap-2">
        <Button variant="quiet" disabled={busy} onClick={onDone}>
          Cancel
        </Button>
        <Button
          variant="accent"
          disabled={busy || name.trim().length === 0}
          aria-busy={busy || undefined}
          onClick={() => void submit()}
        >
          {busy ? 'Creating…' : 'Create'}
        </Button>
      </div>
    </div>
  )
}

const PLUS_ICON = (
  <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-3.5 h-3.5 block">
    <path d="M8 3.5v9M3.5 8h9" />
  </svg>
)

/** Collapse the projects nav to its rail, or open it again. The nav is
 *  never fully hidden (it collapses to the rail), so its toggle sits where
 *  Home's does: « at the right end of the nav's top row ('header': the
 *  drawers' header button in its glyph shape, as Home's « is) and » at the
 *  top of the rail ('rail'), reachable whether or not a project is open. */
export function ProjectNavCollapseButton({ placement = 'rail' }: { placement?: 'header' | 'rail' }): React.JSX.Element {
  const collapsed = useProjectGroupsStore((s) => s.navCollapsed)
  const label = collapsed ? 'Open projects nav' : 'Collapse projects nav'
  const toggle = (): void => useProjectGroupsStore.getState().setNavCollapsed(!collapsed)
  if (placement === 'header') {
    return (
      <DrawerHeaderButton label={label} shape="glyph" aria-expanded={!collapsed} data-projects-nav-toggle="" onClick={toggle}>
        <DoubleChevron direction={collapsed ? 'right' : 'left'} className="w-4 h-4" />
      </DrawerHeaderButton>
    )
  }
  return (
    <button
      type="button"
      className={`k2-focus no-drag flex items-center justify-center ${NAV_TOGGLE_BOX} flex-shrink-0 rounded-[var(--radius-field)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-row-hover)] transition-colors`}
      data-k2-chamfer="control"
      onClick={toggle}
      title={label}
      aria-label={label}
      aria-expanded={!collapsed}
      data-projects-nav-toggle=""
    >
      <DoubleChevron direction={collapsed ? 'right' : 'left'} className="w-4 h-4" />
    </button>
  )
}

// ── Collapsed icon rail (§6.7.1 — the IconRail idiom, group-shaped) ──────

function RailIcon({
  group,
  selected,
  onSelect,
}: {
  group: ProjectGroup
  selected: boolean
  onSelect: () => void
}): React.JSX.Element {
  // Same menu as the expanded row.
  const handleContextMenu = async (e: React.MouseEvent): Promise<void> => {
    e.preventDefault()
    e.stopPropagation()
    await openProjectGroupContextMenu(group, () => setGroupPinned(group), e.currentTarget as HTMLElement)
  }

  return (
    <button
      type="button"
      data-k2-focus=""
      data-rail-group-id={group.id}
      aria-current={selected ? 'true' : undefined}
      aria-label={group.name}
      className={cx(
        'no-drag relative flex items-center justify-center w-9 h-9 flex-shrink-0 outline-none rounded-[var(--radius-field)]',
        selected ? 'bg-[var(--color-accent)]/15' : SETTING_LIST_ROW_HOVER,
      )}
      onClick={onSelect}
      onContextMenu={(e) => void handleContextMenu(e)}
      title={`${group.name} · ${agentCountText(group.memberCount)}`}
    >
      {selected && (
        <span aria-hidden="true" className="absolute -left-1.5 top-1.5 bottom-1.5 w-[3px] bg-[var(--color-accent)]" />
      )}
      <span className={AVATAR_ROUND}>
        <ProjectGroupAvatar name={group.name} groupId={group.id} size={24} color={group.color} />
      </span>
      <ConversationUnreadBadge
        kind="project"
        id={group.id}
        variant="dot"
        className="absolute top-0.5 right-0.5"
        label="project messages"
      />
    </button>
  )
}

/** §6.7.1 — the collapsed Projects nav: a narrow strip of project
 *  pictures (pinned on top, a divider, the rest — the expanded nav's
 *  order), click to select, the name and agent count as the tooltip; then
 *  the selected project's agents as pictures. Creating a project is the
 *  expanded nav's "+"; the project bar's name opens a switcher while the
 *  nav is a rail. */
export function ProjectNavRail({
  show,
  canAddAgents,
  onChanged,
}: {
  show: ProjectGroupShow | null
  canAddAgents: boolean
  onChanged: () => void
}): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const selectGroup = useProjectGroupsStore((s) => s.selectGroup)

  const { pinned, unpinned } = useMemo(() => partitionPinned(groups ?? []), [groups])

  return (
    <nav
      aria-label="Projects"
      data-material="tile"
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] py-2 flex-shrink-0"
      style={{ width: PROJECT_NAV_RAIL_WIDTH }}
    >
      <div className="pb-1.5 flex-shrink-0">
        <ProjectNavCollapseButton />
      </div>
      <div className="flex-shrink min-h-0 flex flex-col items-center gap-1 overflow-y-auto overflow-x-hidden w-full py-0.5">
        {pinned.map((group) => (
          <RailIcon
            key={group.id}
            group={group}
            selected={group.id === selectedGroupId}
            onSelect={() => selectGroup(group.id)}
          />
        ))}
        {pinned.length > 0 && unpinned.length > 0 && (
          <div aria-hidden="true" className="w-6 border-b border-[var(--color-border)] my-1 flex-shrink-0" />
        )}
        {unpinned.map((group) => (
          <RailIcon
            key={group.id}
            group={group}
            selected={group.id === selectedGroupId}
            onSelect={() => selectGroup(group.id)}
          />
        ))}
      </div>
      {show && <RailAgents show={show} canAddAgents={canAddAgents} onChanged={onChanged} />}
    </nav>
  )
}

// ── The nav ───────────────────────────────────────────────────────────────

/** A list section's heading ("Pinned", "All projects", "Agents"). */
export function NavSubheading({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <h3 className="px-4 pt-2 pb-1 type-hint font-semibold text-[var(--color-text-secondary)]">{children}</h3>
}


const LIST_OPEN_KEY = 'k2:projects-nav:list-open'

function readListOpen(): boolean {
  try {
    return localStorage.getItem(LIST_OPEN_KEY) !== '0'
  } catch {
    return true
  }
}

export default function ProjectNav({
  show,
  dashboard,
  canAddAgents,
  canSetAgentViews = true,
  onChanged,
}: {
  /** The selected project's show (null while it loads or failed). */
  show: ProjectGroupShow | null
  /** The dashboard on screen (or the one an open request lands on). */
  dashboard: ProjectGroupDashboard | null
  /** Who may add an agent (project-settings.ts: any login). */
  canAddAgents: boolean
  /** Who may set where an agent opens for everyone (owner or admin). */
  canSetAgentViews?: boolean
  /** Refetch the selected project after a change made in the nav. */
  onChanged: () => void
}): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const selectGroup = useProjectGroupsStore((s) => s.selectGroup)
  const [creating, setCreating] = useState(false)
  // The project list folds to just the selected project (per client).
  const [listOpen, setListOpenRaw] = useState(readListOpen)
  const setListOpen = (v: boolean): void => {
    setListOpenRaw(v)
    try {
      localStorage.setItem(LIST_OPEN_KEY, v ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    const onNew = (): void => setCreating(true)
    window.addEventListener(NEW_PROJECT_EVENT, onNew)
    return () => window.removeEventListener(NEW_PROJECT_EVENT, onNew)
  }, [])

  const { pinned, unpinned } = useMemo(() => partitionPinned(groups ?? []), [groups])
  const selected = groups?.find((g) => g.id === selectedGroupId) ?? null

  // Whether the capped list has rows below its cut (the fade shows then).
  const listRef = useRef<HTMLDivElement>(null)
  const [moreBelow, setMoreBelow] = useState(false)
  const checkMore = (): void => {
    const el = listRef.current
    if (!el) return
    setMoreBelow(el.scrollHeight - el.scrollTop - el.clientHeight > 2)
  }
  useEffect(() => {
    checkMore()
    const el = listRef.current
    if (!el || typeof ResizeObserver !== 'function') return
    const ro = new ResizeObserver(checkMore)
    ro.observe(el)
    return () => ro.disconnect()
  }, [groups, listOpen, show !== null])
  const hasGroups = (groups?.length ?? 0) > 0

  // TODO(V1.1): drag-to-reorder within sections — Sidebar's
  // handleProjectMouseDown threshold + drop-index pattern (needs a
  // sort-order route; P2 shipped pin only).
  const rows = (list: ProjectGroup[]): React.JSX.Element[] =>
    list.map((group) => (
      <GroupRow
        key={group.id}
        group={group}
        selected={group.id === selectedGroupId}
        onSelect={() => selectGroup(group.id)}
      />
    ))

  return (
    <nav
      aria-label="Projects"
      data-projects-nav=""
      className="relative flex flex-col h-full w-full"
    >
      {/* The top row, level with the project bar with its bottom line, so
          the two lines meet across the gap: the list's name, its fold, New
          project and «, centred in the row. */}
      <div
        className="flex items-center gap-1.5 pl-4 flex-shrink-0 border-b border-[var(--color-border)] no-drag"
        style={{ height: PROJECT_BAR_ROW_HEIGHT }}
        data-projects-nav-header=""
      >
        <h2 className="type-heading text-[var(--color-text-primary)] truncate">Projects</h2>
        {hasGroups && (
          <button
            type="button"
            className="k2-focus no-drag flex items-center justify-center w-6 h-6 flex-shrink-0 rounded-[var(--radius-field)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-row-hover)] transition-colors"
            data-k2-chamfer="control"
            aria-expanded={listOpen}
            aria-controls="projects-nav-list"
            aria-label={listOpen ? 'Fold the project list' : 'Show the project list'}
            title={listOpen ? 'Fold the list' : 'Show the list'}
            onClick={() => setListOpen(!listOpen)}
            data-projects-list-toggle=""
          >
            <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 transition-transform" style={{ transform: listOpen ? 'rotate(90deg)' : undefined }}>
              <path d="M6 3.5 10.5 8 6 12.5" />
            </svg>
          </button>
        )}
        <span className="flex-1" />
        <DrawerHeaderButton
          label="New project"
          shape="glyph"
          aria-expanded={creating}
          onClick={() => setCreating((v) => !v)}
          data-new-project=""
        >
          {PLUS_ICON}
        </DrawerHeaderButton>
        <ProjectNavCollapseButton placement="header" />
      </div>
      {creating && (
        <div className="px-4 py-3 flex-shrink-0">
          <CreateProjectForm onDone={() => setCreating(false)} />
        </div>
      )}

      {/* The project list: its own scroll, capped so the selected
          project's section below always has room (1024×768 included).
          Folded, only the selected project's row stays. */}
      <div
        ref={listRef}
        id="projects-nav-list"
        className={cx(
          'overflow-y-auto overflow-x-hidden pb-2',
          show ? 'flex-shrink min-h-[5rem] max-h-[45%]' : 'flex-1',
        )}
        data-projects-list=""
        onScroll={checkMore}
      >
        {listOpen ? (
          <>
            {pinned.length > 0 && (
              <section aria-label="Pinned">
                <NavSubheading>Pinned</NavSubheading>
                <div className="px-1">{rows(pinned)}</div>
              </section>
            )}
            {unpinned.length > 0 && (
              <section aria-label="All projects">
                <NavSubheading>All projects</NavSubheading>
                <div className="px-1">{rows(unpinned)}</div>
              </section>
            )}
          </>
        ) : (
          selected && <div className="px-1">{rows([selected])}</div>
        )}
        {/* Loading, failed or empty: the canvas says so (one state card,
            ProjectStates.tsx); the nav keeps just its header and +. */}
        {/* More projects below the cut: a fade over the last row says the
            list scrolls (it never just ends on a half row). */}
        {moreBelow && (
          <div
            aria-hidden="true"
            data-projects-list-fade=""
            className="sticky -bottom-2 -mt-8 h-8 pointer-events-none bg-gradient-to-b from-transparent to-[var(--color-bg-surface)]"
          />
        )}
      </div>

      {show && (
        <div
          className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden border-t border-[var(--color-border)] pb-2"
          data-project-section-scroll=""
        >
          <ProjectNavSection
            key={show.id}
            show={show}
            dashboard={dashboard}
            canAddAgents={canAddAgents}
            canSetAgentViews={canSetAgentViews}
            onChanged={onChanged}
          />
        </div>
      )}
    </nav>
  )
}
