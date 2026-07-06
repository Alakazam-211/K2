// Projects V1 P4 (prd-projects-v1 §6.1, resolved Q4) — the Projects
// page's LEFT NAV, mimicking Sidebar.tsx's anatomy exactly: canonical
// Pinned section on top (project_groups.pinned — the projects.pinned
// precedent, Sidebar.tsx:698), the unpinned project list below, NO
// Active section, and the bottom-drawer slot (ActiveBar's position)
// holding the SELECTED project's member agents in ActiveBar styling —
// always ALL members, never activity-gated. P5 wired the member rows:
// click opens/focuses the member's canonical pane in the dashboard;
// drag (Sidebar's handleProjectMouseDown threshold pattern, bridged
// via dashboard-dnd.ts) drops it onto a §6.2 drop target. Group-list
// drag-to-reorder awaits a sort-order route (V1.1).

import React, { useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import { showContextMenu } from '@/lib/context-menu'
import { KeyCombo } from '@/components/KeySymbol'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import ProjectGroupAvatar from './ProjectGroupAvatar'
import { completeDashDrag, useDashboardDndStore } from './dashboard-dnd'
import {
  createProjectGroup,
  createErrorMessage,
  partitionPinned,
  pinProjectGroup,
  type ProjectGroup,
  type ProjectGroupMemberInfo,
} from './projects-api'

export const PROJECT_NAV_WIDTH = 280
/** Collapsed-rail width (IconRail's RAIL_WIDTH). */
export const PROJECT_NAV_RAIL_WIDTH = 48

// ── Group row (SingleProjectItem's visual idiom, group-shaped) ────────────

function PinGlyph({ filled }: { filled: boolean }): React.JSX.Element {
  return (
    <svg
      width="10"
      height="10"
      viewBox="0 0 24 24"
      fill={filled ? 'currentColor' : 'none'}
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M12 17v5" />
      <path d="M9 3h6l-1 7 3 3H7l3-3-1-7z" />
    </svg>
  )
}

function GroupRow({
  group,
  selected,
  unread,
  onSelect,
}: {
  group: ProjectGroup
  selected: boolean
  unread: boolean
  onSelect: () => void
}): React.JSX.Element {
  const [hovered, setHovered] = useState(false)
  const [pinBusy, setPinBusy] = useState(false)

  const togglePin = async (e?: React.MouseEvent): Promise<void> => {
    e?.stopPropagation()
    if (pinBusy) return
    setPinBusy(true)
    try {
      await pinProjectGroup(group.id, !group.pinned)
      // groups-changed also fires; refetch now so the row moves sections
      // without waiting out the coalesce window.
      await useProjectGroupsStore.getState().fetchGroups()
    } catch (err) {
      useToastStore
        .getState()
        .addToast(
          `${group.pinned ? 'Unpin' : 'Pin'} failed: ${err instanceof Error ? err.message : String(err)}`,
          'error',
        )
    } finally {
      setPinBusy(false)
    }
  }

  // Right-click menu (§6.5) — the Sidebar workspace-row idiom
  // (showContextMenu): "Project Settings" deep-links to Settings →
  // Projects with THIS project preselected; Pin/Unpin mirrors the
  // hover pin button (which stays).
  const handleContextMenu = async (e: React.MouseEvent): Promise<void> => {
    e.preventDefault()
    const clickedId = await showContextMenu([
      { id: 'group-settings', label: 'Project Settings' },
      { id: 'separator-pin', label: '', type: 'separator' },
      { id: 'toggle-pin', label: group.pinned ? 'Unpin' : 'Pin to Top' },
    ])
    if (clickedId === 'group-settings') {
      useSettingsStore.getState().openSettings('project-groups', group.id)
    } else if (clickedId === 'toggle-pin') {
      await togglePin()
    }
  }

  return (
    <div className="no-drag" data-group-id={group.id}>
      <button
        onContextMenu={(e) => void handleContextMenu(e)}
        className={`w-full flex items-stretch gap-2.5 px-3 py-2 text-left text-sm transition-colors ${
          selected
            ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
            : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
        }`}
        style={{
          borderLeft: selected ? '2px solid var(--color-accent)' : '2px solid transparent',
        }}
        onClick={onSelect}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <div className="flex items-center flex-shrink-0">
          {/* §6.7.1/§6.7.7 — icon when set, else initials on the group's
              canonical color / stable hashed pick (the shared avatar
              component the collapsed rail also renders). */}
          <ProjectGroupAvatar
            name={group.name}
            groupId={group.id}
            size={32}
            color={group.color}
          />
        </div>
        <div className="flex flex-col justify-center min-w-0 flex-1">
          <div className="flex items-center gap-2 w-full">
            <span className="truncate flex-1">{group.name}</span>
            {unread && (
              <span
                className="flex-shrink-0 w-1.5 h-1.5 rounded-full bg-[var(--color-accent)]"
                title="New project messages"
              />
            )}
            {(hovered || group.pinned) && (
              <span
                onClick={(e) => void togglePin(e)}
                className={`flex-shrink-0 flex h-4 w-4 items-center justify-center transition-colors cursor-pointer ${
                  group.pinned
                    ? 'text-[var(--color-accent)] hover:text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                } ${pinBusy ? 'opacity-50' : ''}`}
                title={group.pinned ? 'Unpin' : 'Pin to top'}
              >
                <PinGlyph filled={group.pinned} />
              </span>
            )}
          </div>
          <div className="flex items-center gap-1">
            <span className="text-[10px] font-mono text-[var(--color-text-muted)]">
              {group.memberCount} {group.memberCount === 1 ? 'member' : 'members'}
            </span>
          </div>
        </div>
      </button>
    </div>
  )
}

// ── Member drawer (the ActiveBar slot — §6.1, resolved Q4/Q18) ────────────

function MemberRow({
  member,
  isPoc,
}: {
  member: ProjectGroupMemberInfo
  isPoc: boolean
}): React.JSX.Element {
  // Icon/color come from the workspace-registry store row (the member
  // enrichment carries name/path only).
  const workspace = useProjectsStore((s) => s.projects.find((p) => p.id === member.workspaceId))
  const displayName = member.agentName ?? member.name ?? member.workspaceId.slice(0, 8)
  // ⌘N — this member's pane number on the current dashboard (undefined
  // when it has no terminal pane there / no dashboard is mounted).
  const paneNum = useProjectGroupsStore((s) => s.dashPaneNumbers[member.workspaceId])
  // Presence read-only signal (window-mode.ts): viewers can still CLICK
  // (focus an existing pane — the dashboard decides), but never DRAG.
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  // A completed drag fires a trailing click on the same button —
  // swallow exactly that one.
  const suppressClickRef = useRef(false)

  const handleClick = (): void => {
    if (suppressClickRef.current) {
      suppressClickRef.current = false
      return
    }
    // P5 (§6.1/§6.2): open (or focus — one pane per agent) this
    // member's canonical terminal pane in the dashboard. The mounted
    // ProjectDashboard consumes the request.
    useProjectGroupsStore.getState().requestMemberPane(member.workspaceId)
  }

  // P5 (§6.2) — drag a member into the dashboard: Sidebar's
  // handleProjectMouseDown threshold pattern; the pointer streams into
  // the dashboard-dnd store (the dashboard renders drop-target hints)
  // and mouseup offers the drop to the mounted dashboard.
  const handleMouseDown = (e: React.MouseEvent): void => {
    if (readOnly || e.button !== 0) return
    const startX = e.clientX
    const startY = e.clientY
    let started = false

    const handleMouseMove = (ev: MouseEvent): void => {
      if (!started && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 5)) {
        started = true
        suppressClickRef.current = true
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }
      if (!started) return
      useDashboardDndStore.getState().setDrag({
        payload: { type: 'member', workspaceId: member.workspaceId },
        x: ev.clientX,
        y: ev.clientY,
      })
    }

    const handleMouseUp = (ev: MouseEvent): void => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      if (!started) return
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      completeDashDrag({ type: 'member', workspaceId: member.workspaceId }, ev.clientX, ev.clientY)
      useDashboardDndStore.getState().setDrag(null)
      // The trailing click (mousedown+mouseup on this button) is
      // swallowed by the flag; re-arm on the next tick in case the
      // browser skips it (pointer released off-button).
      setTimeout(() => {
        suppressClickRef.current = false
      }, 0)
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }

  return (
    <button
      onClick={handleClick}
      onMouseDown={handleMouseDown}
      className={`no-drag w-full flex items-center gap-2 px-2 py-1 text-left transition-colors select-none text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)] ${
        readOnly ? 'cursor-default' : 'cursor-pointer'
      }`}
      title={member.path ?? undefined}
    >
      <ProjectAvatar
        projectPath={member.path ?? ''}
        projectName={member.name ?? displayName}
        projectColor={workspace?.color ?? 'var(--color-text-muted)'}
        projectId={member.workspaceId}
        iconUrl={workspace?.iconUrl ?? null}
        size={18}
      />
      <span className="text-[11px] truncate flex-1">{displayName}</span>
      {isPoc && (
        <span
          className="flex-shrink-0 px-1.5 py-0.5 text-[8px] font-bold uppercase tracking-wide bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
          title="Point of Contact — project chat is injected into this agent's session"
        >
          PoC
        </span>
      )}
      {paneNum !== undefined && (
        <span
          className="flex-shrink-0 text-[10px] font-mono text-[var(--color-text-muted)] tabular-nums"
          title={`⌘${paneNum} focuses this pane`}
        >
          {paneNum}
        </span>
      )}
    </button>
  )
}

function MemberDrawer({
  members,
  pocWorkspaceId,
}: {
  members: ProjectGroupMemberInfo[]
  pocWorkspaceId: string | null
}): React.JSX.Element | null {
  const [collapsed, setCollapsed] = useState(false)
  if (members.length === 0) return null

  return (
    <div className="border-t border-[var(--color-border)] flex flex-col">
      <button
        className="no-drag w-full flex items-center gap-1.5 px-3 pt-2 pb-1 text-left cursor-pointer hover:bg-white/[0.02] transition-colors"
        onClick={() => setCollapsed((prev) => !prev)}
      >
        <span className="text-[10px] font-semibold tracking-wider text-[var(--color-text-muted)] uppercase">
          Members
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono">
          {members.length}
        </span>
        <span className="text-[9px] font-mono text-[var(--color-text-muted)] opacity-50">
          <KeyCombo combo="⌘ 1-9" />
        </span>
        <span className="flex-1" />
        <svg
          className="w-2.5 h-2.5 text-[var(--color-text-muted)] flex-shrink-0"
          style={{ transition: 'transform 0.2s ease', transform: collapsed ? 'rotate(0deg)' : 'rotate(90deg)' }}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
      </button>
      <div
        style={{
          overflow: 'hidden',
          maxHeight: collapsed ? 0 : 320,
          transition: 'max-height 0.2s ease',
        }}
      >
        <div className="px-1 pb-1 overflow-y-auto" style={{ maxHeight: 320 }}>
          {members.map((m) => (
            <MemberRow key={m.workspaceId} member={m} isPoc={m.workspaceId === pocWorkspaceId} />
          ))}
        </div>
      </div>
    </div>
  )
}

// ── "+ New project" (inline create, surfaces name_taken — §6.1) ──────────

/** Inline name input + Create. POSTs `project-group/create`, surfaces
 *  `name_taken` inline, refetches + selects the new group on success.
 *  Shared by the nav affordance and the page's no-projects empty state. */
export function CreateProjectForm({ onDone }: { onDone: () => void }): React.JSX.Element {
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (): Promise<void> => {
    const trimmed = name.trim()
    if (!trimmed || busy) return
    setBusy(true)
    try {
      const group = await createProjectGroup(trimmed)
      const store = useProjectGroupsStore.getState()
      await store.fetchGroups()
      store.selectGroup(group.id)
      onDone()
    } catch (err) {
      setError(createErrorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      <input
        autoFocus
        type="text"
        value={name}
        disabled={busy}
        onChange={(e) => {
          setName(e.target.value)
          setError(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void submit()
          // stopPropagation so the page-level Esc (back to Agents)
          // doesn't also fire — Esc here just cancels the input.
          if (e.key === 'Escape') {
            e.stopPropagation()
            onDone()
          }
        }}
        placeholder="Project name…"
        className="w-full px-2.5 py-1.5 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)] disabled:opacity-50"
      />
      {error && <p className="text-[10px] text-red-400 leading-snug">{error}</p>}
      <div className="flex items-center gap-1.5">
        <button
          className="flex-1 px-2 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-accent)] hover:bg-[var(--color-accent)]/25 transition-colors disabled:opacity-50"
          disabled={busy || name.trim().length === 0}
          onClick={() => void submit()}
        >
          Create
        </button>
        <button
          className="px-2 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
          disabled={busy}
          onClick={onDone}
        >
          Cancel
        </button>
      </div>
    </div>
  )
}

function NewProjectAffordance(): React.JSX.Element {
  const [editing, setEditing] = useState(false)

  if (!editing) {
    return (
      <div className="p-3 border-t border-[var(--color-border)]">
        <button
          className="no-drag w-full flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] transition-colors"
          onClick={() => setEditing(true)}
        >
          <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
          </svg>
          New Project
        </button>
      </div>
    )
  }

  return (
    <div className="p-3 border-t border-[var(--color-border)]">
      <CreateProjectForm onDone={() => setEditing(false)} />
    </div>
  )
}

// ── Collapsed icon rail (§6.7.1 — the IconRail idiom, group-shaped) ──────

function RailIcon({
  group,
  selected,
  unread,
  onSelect,
}: {
  group: ProjectGroup
  selected: boolean
  unread: boolean
  onSelect: () => void
}): React.JSX.Element {
  return (
    <button
      type="button"
      className={`no-drag relative flex items-center justify-center w-8 h-8 flex-shrink-0 transition-colors ${
        selected
          ? 'bg-white/[0.12]'
          : 'hover:bg-white/[0.06]'
      }`}
      onClick={onSelect}
      title={`${group.name} • ${group.memberCount} ${group.memberCount === 1 ? 'member' : 'members'}`}
    >
      <ProjectGroupAvatar name={group.name} groupId={group.id} size={20} color={group.color} />
      {unread && (
        <span
          className="absolute top-0.5 right-0.5 w-1.5 h-1.5 rounded-full bg-[var(--color-accent)]"
          title="New project messages"
        />
      )}
    </button>
  )
}

/** The member drawer's rail form: one avatar per member of the
 *  selected project (IconRail's bottom Active-zone anatomy), click =
 *  the SAME open-canonical-pane action as the expanded drawer's row,
 *  tooltip = agent name (+ PoC), ⌘N overlay when the member's terminal
 *  pane is on the current dashboard (the IconRail shortcutIndex
 *  idiom). */
function RailMember({
  member,
  isPoc,
}: {
  member: ProjectGroupMemberInfo
  isPoc: boolean
}): React.JSX.Element {
  const workspace = useProjectsStore((s) => s.projects.find((p) => p.id === member.workspaceId))
  const paneNum = useProjectGroupsStore((s) => s.dashPaneNumbers[member.workspaceId])
  const displayName = member.agentName ?? member.name ?? member.workspaceId.slice(0, 8)

  return (
    <button
      type="button"
      className="no-drag relative flex items-center justify-center w-8 h-8 flex-shrink-0 hover:bg-white/[0.06] transition-colors cursor-pointer"
      onClick={() => useProjectGroupsStore.getState().requestMemberPane(member.workspaceId)}
      title={isPoc ? `${displayName} • PoC` : displayName}
    >
      <ProjectAvatar
        projectPath={member.path ?? ''}
        projectName={member.name ?? displayName}
        projectColor={workspace?.color ?? 'var(--color-text-muted)'}
        projectId={member.workspaceId}
        iconUrl={workspace?.iconUrl ?? null}
        size={18}
      />
      {paneNum !== undefined && (
        <span className="absolute bottom-0 right-0.5 text-[8px] font-mono text-[var(--color-text-muted)] tabular-nums leading-none">
          {paneNum}
        </span>
      )}
    </button>
  )
}

/** §6.7.1 — the collapsed Projects nav: a narrow vertical strip of
 *  group avatars (IconRail's anatomy: pinned zone on top, divider,
 *  unpinned list — the SAME order as the expanded nav), click to
 *  select, tooltip with the name. The selected project's MEMBERS keep
 *  a bottom zone after a divider (IconRail's Active-zone anatomy —
 *  the member drawer in rail form); the create affordance lives only
 *  in the expanded nav. */
export function ProjectNavRail({
  members,
  pocWorkspaceId,
}: {
  /** Members of the SELECTED project (the page owns the `show` fetch). */
  members: ProjectGroupMemberInfo[] | null
  pocWorkspaceId: string | null
}): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const selectGroup = useProjectGroupsStore((s) => s.selectGroup)
  const unreadGroupIds = useProjectGroupsStore((s) => s.unreadGroupIds)

  const { pinned, unpinned } = useMemo(() => partitionPinned(groups ?? []), [groups])

  return (
    <div
      className="flex flex-col items-center h-full bg-[var(--color-bg-surface)] border-r border-[var(--color-border)] py-2 flex-shrink-0"
      style={{ width: PROJECT_NAV_RAIL_WIDTH }}
    >
      {pinned.length > 0 && (
        <div className="flex flex-col items-center gap-0.5 pb-1.5 w-full">
          {pinned.map((group) => (
            <RailIcon
              key={group.id}
              group={group}
              selected={group.id === selectedGroupId}
              unread={unreadGroupIds.has(group.id)}
              onSelect={() => selectGroup(group.id)}
            />
          ))}
          <div className="w-6 border-b border-[var(--color-border)] mt-1" />
        </div>
      )}

      <div className="flex-1 flex flex-col items-center gap-0.5 overflow-y-auto overflow-x-hidden w-full">
        {unpinned.map((group) => (
          <RailIcon
            key={group.id}
            group={group}
            selected={group.id === selectedGroupId}
            unread={unreadGroupIds.has(group.id)}
            onSelect={() => selectGroup(group.id)}
          />
        ))}
      </div>

      {/* Bottom zone (IconRail's Active-zone anatomy): the selected
          project's members — the expanded nav's drawer in rail form,
          same gating (a selected project with a resolved `show`). */}
      {selectedGroupId && members && members.length > 0 && (
        <div className="flex flex-col items-center w-full flex-shrink-0 pt-1.5">
          <div className="w-6 border-b border-[var(--color-border)] mb-1" />
          {/* Same 320px scroll cap as the expanded drawer's list. */}
          <div
            className="flex flex-col items-center gap-0.5 w-full overflow-y-auto overflow-x-hidden"
            style={{ maxHeight: 320 }}
          >
            {members.map((m) => (
              <RailMember
                key={m.workspaceId}
                member={m}
                isPoc={m.workspaceId === pocWorkspaceId}
              />
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

// ── The nav ───────────────────────────────────────────────────────────────

export default function ProjectNav({
  members,
  pocWorkspaceId,
}: {
  /** Members of the SELECTED project (the page owns the `show` fetch). */
  members: ProjectGroupMemberInfo[] | null
  pocWorkspaceId: string | null
}): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const selectGroup = useProjectGroupsStore((s) => s.selectGroup)
  const unreadGroupIds = useProjectGroupsStore((s) => s.unreadGroupIds)

  // Pinned-section collapse persists across launches (Sidebar's
  // agentsPinnedCollapsed idiom).
  const [pinnedCollapsed, setPinnedCollapsedRaw] = useState<boolean>(() => {
    try {
      return localStorage.getItem('k2:projects-nav:pinnedCollapsed') === '1'
    } catch {
      return false
    }
  })
  const setPinnedCollapsed = (v: boolean): void => {
    setPinnedCollapsedRaw(v)
    try {
      localStorage.setItem('k2:projects-nav:pinnedCollapsed', v ? '1' : '0')
    } catch {
      /* ignore */
    }
  }

  const { pinned, unpinned } = useMemo(() => partitionPinned(groups ?? []), [groups])

  return (
    <div
      className="relative flex flex-col h-full flex-shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-surface)]"
      style={{ width: PROJECT_NAV_WIDTH }}
    >
      {/* Pinned projects — canonical section, always on top (Q4). */}
      {pinned.length > 0 && (
        <div className="border-b border-[var(--color-border)]">
          <button
            type="button"
            onClick={() => setPinnedCollapsed(!pinnedCollapsed)}
            className="no-drag w-full flex items-center gap-1.5 px-4 pt-3 pb-2 text-left cursor-pointer hover:bg-white/[0.02] transition-colors"
            aria-expanded={!pinnedCollapsed}
          >
            <span className="text-[10px] font-semibold tracking-wider text-[var(--color-text-muted)] uppercase">
              Pinned
            </span>
            <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono">
              {pinned.length}
            </span>
            <span className="flex-1" />
            <svg
              className="w-2.5 h-2.5 text-[var(--color-text-muted)] flex-shrink-0"
              style={{ transition: 'transform 0.2s ease', transform: pinnedCollapsed ? 'rotate(0deg)' : 'rotate(90deg)' }}
              fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
            </svg>
          </button>
          <div
            style={{
              overflow: 'hidden',
              maxHeight: pinnedCollapsed ? 0 : 1200,
              transition: 'max-height 0.2s ease',
            }}
          >
            {/* TODO(V1.1): drag-to-reorder within sections — Sidebar's
                handleProjectMouseDown threshold + drop-index pattern
                (needs a sort-order route; P2 shipped pin only). */}
            {pinned.map((group) => (
              <GroupRow
                key={group.id}
                group={group}
                selected={group.id === selectedGroupId}
                unread={unreadGroupIds.has(group.id)}
                onSelect={() => selectGroup(group.id)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Section header (FocusGroupSelector's plain-label idiom). */}
      <div className="px-4 pt-3 pb-2 no-drag">
        <span className="text-[11px] font-semibold tracking-wide text-[var(--color-text-muted)] uppercase">
          Projects
        </span>
      </div>

      {/* Unpinned project list */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden py-1">
        {unpinned.map((group, idx) => (
          <React.Fragment key={group.id}>
            <GroupRow
              group={group}
              selected={group.id === selectedGroupId}
              unread={unreadGroupIds.has(group.id)}
              onSelect={() => selectGroup(group.id)}
            />
            {idx < unpinned.length - 1 && <div className="border-b border-[var(--color-border)]" />}
          </React.Fragment>
        ))}

        {groups !== null && groups.length === 0 && (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">No projects yet</p>
            <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-60">
              Create one to group agents into an effort
            </p>
          </div>
        )}
        {groups === null && (
          <div className="px-3 py-6 text-center">
            <p className="text-xs text-[var(--color-text-muted)]">Loading projects…</p>
          </div>
        )}
      </div>

      {/* Bottom-drawer slot (ActiveBar's position): the selected
          project's members — ALL of them, never activity-gated. */}
      {selectedGroupId && members && (
        <MemberDrawer members={members} pocWorkspaceId={pocWorkspaceId} />
      )}

      <NewProjectAffordance />
    </div>
  )
}
