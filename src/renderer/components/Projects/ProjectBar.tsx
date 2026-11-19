// The project bar (projects-reorg-draft §3.2): one row at the top of the
// canvas, in place of the old dashboard tab row, that says which project
// this is and holds every action for it, most used first.
//
//   left   = identity: picture, name, one quiet line ("6 agents · Point of
//            contact: Atlas"). While the nav is a rail the name opens a
//            project switcher.
//   middle = the dashboards as tabs; "+" adds one (owners and admins) and
//            leaves its tab ready to rename. Any tab's menu (right-click,
//            or Shift+F10 / the context-menu key on a focused tab) has
//            Rename and Delete (with the shared confirm); F2 or a
//            double-click renames. Nothing sits beside a tab, so switching
//            tabs never moves one. A divider, then Feedback, which is a
//            different kind of page (owner call Q2).
//   right  = Layout (the presets menu), Settings ⚙ (the project's
//            settings, as the nav's right-click opens them), Chat (opens the
//            chat, with the unread count; hidden while the chat is open,
//            whose header × closes it).
//
// Who sees what follows project-settings.ts (the login's role, never the
// window's eye/pencil): dashboards and their layout are owner or admin.
// A Member login sees no "+", no tab menus and no Layout. The project's
// agents and their files are in the nav, under the project list
// (ProjectAgentsSection.tsx). When the bar gets narrow (chat open on a
// small window) Layout and Chat drop their words and keep their icons.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import { useUnreadEntry } from '@/stores/unread'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { showContextMenu } from '@/lib/context-menu'
import { Button, Input, SearchField, cx } from '@/components/ui'
import { ConversationUnreadBadge } from '@/components/common/UnreadBadge'
import { AVATAR_ROUND } from '@/components/Settings/controls/SettingControls'
import { ConfirmSheet } from '@/components/Settings/sections/AccessDialogs'
import { SettingsIcon, TicketsIcon } from '@/components/icons/K2Icons'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { primaryScope } from '@/kessel/server-scope'
import { clientToCssPx } from '@/lib/app-zoom'
import ProjectGroupAvatar from './ProjectGroupAvatar'
import DashboardPresetsMenu from './DashboardPresetsMenu'
import { memberDisplayName } from './ProjectAgentsSection'
import { BarMenuItem, BarMenuPanel, Chevron, useBarMenu } from './bar-menu'
import { FEEDBACK_TAB } from './project-tabs'
import { agentCountText, filterGroupsByQuery, projectSettingsCaps, type ProjectRole } from './project-settings'
import { openProjectSettings } from './ProjectSettingsSheet'
import { NEW_PROJECT_EVENT } from './project-nav-events'
import { PROJECT_BAR_HEIGHT_PX } from './project-bar-height'
import {
  createErrorMessage,
  createProjectGroupDashboard,
  daemonErrorInfo,
  deleteProjectGroupDashboard,
  partitionPinned,
  renameProjectGroupDashboard,
  type ProjectGroupDashboard,
  type ProjectGroupShow,
} from './projects-api'

/** Below this bar width the right-hand buttons keep only their icons… */
const COMPACT_BAR_PX = 760
/** …and below this one the project shows only its picture (name and line
 *  in its tooltip), so the dashboard tabs keep room. */
const TINY_BAR_PX = 620

/** A free name for a new dashboard: "New dashboard", then "New dashboard 2"… */
export function nextDashboardName(existing: string[]): string {
  const taken = new Set(existing.map((n) => n.trim().toLowerCase()))
  if (!taken.has('new dashboard')) return 'New dashboard'
  for (let i = 2; ; i++) {
    const name = `New dashboard ${i}`
    if (!taken.has(name.toLowerCase())) return name
  }
}

/** "6 agents · Point of contact: Atlas" (the point of contact only when set). */
export function projectSummary(show: ProjectGroupShow): string {
  const poc = show.members.find((m) => m.workspaceId === show.pocWorkspaceId)
  const count = agentCountText(show.members.length)
  return poc ? `${count} · Point of contact: ${memberDisplayName(poc)}` : count
}

/** The signed-in login's role (null until known: the daemon decides). */
export function useViewerRole(): ProjectRole | null {
  const [role, setRole] = useState<ProjectRole | null>(null)
  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((r) => {
      if (!cancelled) setRole(r)
    })
    return () => {
      cancelled = true
    }
  }, [])
  return role
}

/** The bar's width, live (0 until measured), to fit it when it's tight. */
function useBarWidth(ref: React.RefObject<HTMLDivElement | null>): number {
  const [width, setWidth] = useState(0)
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver !== 'function') return
    // CSS px, like COMPACT_BAR_PX and TINY_BAR_PX: a rect is on-screen px,
    // which the app zoom scales (lib/app-zoom).
    const ro = new ResizeObserver(() => setWidth(clientToCssPx(el.getBoundingClientRect().width)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref])
  return width
}

/** Whether the tabs fit: the width they'd take (measured from a hidden
 *  copy of the row) against the room the bar gives them. Unmeasured (0)
 *  counts as fitting. */
function useTabsFit(
  room: React.RefObject<HTMLDivElement | null>,
  measure: React.RefObject<HTMLDivElement | null>,
  deps: unknown[],
): boolean {
  const [fits, setFits] = useState(true)
  useEffect(() => {
    const box = room.current
    const copy = measure.current
    if (!box || !copy) return
    const check = (): void => {
      const avail = box.clientWidth
      setFits(avail === 0 || copy.scrollWidth <= avail)
    }
    check()
    if (typeof ResizeObserver !== 'function') return
    const ro = new ResizeObserver(check)
    ro.observe(box)
    return () => ro.disconnect()
  }, [room, measure, ...deps])
  return fits
}

/** When the tabs don't fit: one button naming the open tab, whose menu
 *  lists every dashboard, then Feedback after a separator. */
function TabPicker({
  dashboards,
  activeTab,
  onTab,
  menuProps,
}: {
  dashboards: ProjectGroupDashboard[]
  activeTab: string | null
  onTab: (id: string) => void
  /** The open dashboard's menu on the picker (right-click, keys). */
  menuProps: TabMenuProps
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const menu = useBarMenu(open, setOpen, { minWidth: 220, align: 'start', estimatedHeight: 60 + dashboards.length * 30 })
  const current =
    activeTab === FEEDBACK_TAB ? 'Feedback' : (dashboards.find((d) => d.id === activeTab)?.name ?? 'Dashboards')
  return (
    <>
      <button
        ref={menu.anchorRef}
        type="button"
        data-k2-focus=""
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Dashboards and Feedback: ${current} is open`}
        title="Dashboards and Feedback"
        onClick={() => setOpen(!open)}
        onContextMenu={menuProps.onContextMenu}
        onKeyDown={menuProps.onKeyDown}
        className={cx(tabClass(true), 'min-w-0 gap-1')}
        data-tab-picker=""
      >
        {activeTab === FEEDBACK_TAB && <TicketsIcon size={13} className="flex-shrink-0" />}
        <span className="truncate">{current}</span>
        <Chevron open={open} />
      </button>
      {open && (
        <BarMenuPanel menu={menu} label="Dashboards and Feedback" onClose={() => setOpen(false)} testId="tab-picker-menu" className="max-h-[min(calc(var(--k2-vh)*0.7),30rem)] overflow-y-auto">
          <div className="px-3 pt-1.5 pb-1 type-hint font-semibold text-[var(--color-text-secondary)]" role="presentation">
            Dashboards
          </div>
          {dashboards.map((d) => (
            <BarMenuItem
              key={d.id}
              selected={d.id === activeTab}
              onSelect={() => {
                setOpen(false)
                onTab(d.id)
              }}
            >
              <span className="truncate">{d.name}</span>
            </BarMenuItem>
          ))}
          <div role="separator" className="my-1 border-t border-[var(--color-border)]" />
          <BarMenuItem
            selected={activeTab === FEEDBACK_TAB}
            onSelect={() => {
              setOpen(false)
              onTab(FEEDBACK_TAB)
            }}
          >
            <TicketsIcon size={13} className="flex-shrink-0" />
            Feedback
          </BarMenuItem>
        </BarMenuPanel>
      )}
    </>
  )
}

// ── Identity (and the switcher while the nav is a rail) ─────────────────

/** The project's name as a button that opens a switcher: "Find a
 *  project", Pinned, All projects, then New project. Shown while the nav is
 *  a rail or the bar is narrow; `compact` keeps only the chevron. */
function ProjectSwitcher({ show, compact }: { show: ProjectGroupShow; compact: boolean }): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const menu = useBarMenu(open, setOpen, { minWidth: 260, align: 'start', estimatedHeight: 360 })
  const { pinned, unpinned } = useMemo(
    () => partitionPinned(filterGroupsByQuery(groups ?? [], query)),
    [groups, query],
  )
  useEffect(() => {
    if (!open) setQuery('')
  }, [open])

  const item = (g: NonNullable<typeof groups>[number]): React.JSX.Element => (
    <BarMenuItem
      key={g.id}
      selected={g.id === show.id}
      onSelect={() => {
        setOpen(false)
        useProjectGroupsStore.getState().selectGroup(g.id)
      }}
    >
      <span className={AVATAR_ROUND}>
        <ProjectGroupAvatar name={g.name} groupId={g.id} size={20} color={g.color} />
      </span>
      <span className="min-w-0 flex-1 truncate">{g.name}</span>
      <ConversationUnreadBadge kind="project" id={g.id} variant="dot" label="project messages" />
      {g.id === show.id && (
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3.5 h-3.5 flex-shrink-0 text-[var(--color-accent-text)]">
          <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
        </svg>
      )}
    </BarMenuItem>
  )

  const heading = (text: string): React.JSX.Element => (
    <div className="px-3 pt-2 pb-1 type-hint font-semibold text-[var(--color-text-secondary)]" role="presentation">
      {text}
    </div>
  )

  return (
    <>
      <button
        ref={menu.anchorRef}
        type="button"
        data-k2-focus=""
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={compact ? `Switch project (now ${show.name})` : undefined}
        title="Switch project"
        onClick={() => setOpen(!open)}
        className="relative flex items-center gap-1 min-w-0 max-w-full no-drag outline-none cursor-pointer text-[var(--color-text-primary)] hover:text-[var(--color-accent-text)] rounded-[var(--radius-selector)]"
        data-project-switcher=""
      >
        {!compact && <span className="type-heading truncate">{show.name}</span>}
        <Chevron open={open} />
      </button>
      {open && (
        <BarMenuPanel
          menu={menu}
          label="Switch project"
          onClose={() => setOpen(false)}
          testId="project-switcher"
          focusFirst="[data-switcher-find]"
          className="max-h-[min(calc(var(--k2-vh)*0.7),30rem)] overflow-y-auto w-[17rem]"
        >
          <div className="px-2 pt-1 pb-1.5">
            <SearchField
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onClear={() => setQuery('')}
              aria-label="Find a project"
              placeholder="Find a project"
              className="w-full"
              data-switcher-find=""
            />
          </div>
          {pinned.length > 0 && heading('Pinned')}
          {pinned.map(item)}
          {unpinned.length > 0 && heading('All projects')}
          {unpinned.map(item)}
          {pinned.length + unpinned.length === 0 && (
            <p className="px-3 py-1.5 type-hint text-[var(--color-text-secondary)]">No project matches.</p>
          )}
          <div role="separator" className="my-1 border-t border-[var(--color-border)]" />
          <BarMenuItem
            onSelect={() => {
              setOpen(false)
              // The create form lives in the nav: open it there.
              useProjectGroupsStore.getState().setNavCollapsed(false)
              setTimeout(() => window.dispatchEvent(new Event(NEW_PROJECT_EVENT)), 0)
            }}
            testId="switcher-new-project"
          >
            <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-3.5 h-3.5 flex-shrink-0">
              <path d="M8 3.5v9M3.5 8h9" />
            </svg>
            New project
          </BarMenuItem>
        </BarMenuPanel>
      )}
    </>
  )
}

function Identity({
  show,
  switcher,
  tiny,
}: {
  show: ProjectGroupShow
  /** The name opens the project switcher (nav as a rail, or a narrow bar). */
  switcher: boolean
  /** Only the picture and the switcher's chevron. */
  tiny: boolean
}): React.JSX.Element {
  const summary = projectSummary(show)
  return (
    <div
      className={cx('flex items-center gap-2.5 min-w-0 flex-shrink', tiny ? 'max-w-[6rem]' : switcher ? 'max-w-[12rem]' : 'max-w-[17rem]')}
      data-project-identity=""
      title={tiny ? `${show.name}: ${summary}` : undefined}
    >
      <span className={AVATAR_ROUND}>
        <ProjectGroupAvatar name={show.name} groupId={show.id} size={32} color={show.color} />
      </span>
      <div className="min-w-0 flex flex-col">
        {switcher ? (
          <ProjectSwitcher show={show} compact={tiny} />
        ) : (
          <h1 className="type-heading text-[var(--color-text-primary)] truncate">{show.name}</h1>
        )}
        <p className={cx('type-hint text-[var(--color-text-secondary)] truncate', tiny ? 'sr-only' : '')} title={summary} data-project-summary="">
          {summary}
        </p>
      </div>
    </div>
  )
}

// ── Dashboard tabs ────────────────────────────────────────────────────────

const TAB =
  'relative flex items-center px-3 type-control whitespace-nowrap border-b-2 -mb-px transition-colors no-drag cursor-pointer outline-none'

/** What opens a dashboard's menu: right-click, Shift+F10 or the
 *  context-menu key; F2 renames. */
interface TabMenuProps {
  onContextMenu: (e: React.MouseEvent<HTMLElement>) => void
  onKeyDown: (e: React.KeyboardEvent<HTMLElement>) => void
}

function tabClass(active: boolean): string {
  return cx(
    TAB,
    active
      ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
      : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
  )
}

/** A tab's name as an inline field (after "+", or Rename). Enter or a click
 *  away keeps it; Escape keeps the old name. */
function RenameField({
  dashboard,
  onDone,
}: {
  dashboard: ProjectGroupDashboard
  onDone: (name: string | null) => void
}): React.JSX.Element {
  const [value, setValue] = useState(dashboard.name)
  const ref = useRef<HTMLInputElement>(null)
  const done = useRef(false)
  useEffect(() => {
    ref.current?.focus()
    ref.current?.select()
  }, [])
  const finish = (name: string | null): void => {
    if (done.current) return
    done.current = true
    onDone(name)
  }
  return (
    <div className="flex items-center px-1 border-b-2 border-[var(--color-accent)] -mb-px">
      <Input
        ref={ref}
        aria-label="Dashboard name"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            finish(value)
          } else if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            finish(null)
          }
        }}
        onBlur={() => finish(value)}
        className="w-40"
        data-dashboard-rename=""
      />
    </div>
  )
}

function DashboardTabs({
  show,
  dashboards,
  activeTab,
  onTab,
  canEdit,
  compact,
  onChanged,
}: {
  show: ProjectGroupShow
  dashboards: ProjectGroupDashboard[]
  activeTab: string | null
  onTab: (id: string | null) => void
  canEdit: boolean
  /** A narrow bar (the fit check runs again when it changes). */
  compact: boolean
  onChanged: () => void
}): React.JSX.Element {
  const [editing, setEditing] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState<ProjectGroupDashboard | null>(null)
  const plusRef = useRef<HTMLButtonElement>(null)
  const roomRef = useRef<HTMLDivElement>(null)
  const measureRef = useRef<HTMLDivElement>(null)
  const names = dashboards.map((d) => d.name).join('\n')
  // The tabs show as tabs only when every one fits whole, with + and
  // Feedback; otherwise one picker holds them all (Feedback included), so
  // nothing is ever cut or hidden.
  const fits = useTabsFit(roomRef, measureRef, [names, canEdit, compact])
  const toast = (text: string, kind: 'success' | 'error'): void => useToastStore.getState().addToast(text, kind)
  const lastOne = dashboards.length <= 1

  const focusTab = (id: string | null): void => {
    requestAnimationFrame(() => {
      const el =
        (id ? document.querySelector<HTMLElement>(`[data-dashboard-tab="${id}"] [role="tab"]`) : null) ??
        document.querySelector<HTMLElement>('[data-tab-picker]')
      el?.focus({ preventScroll: true })
    })
  }

  const create = async (): Promise<void> => {
    if (creating) return
    setCreating(true)
    try {
      const d = await createProjectGroupDashboard(primaryScope(), show.id, nextDashboardName(dashboards.map((x) => x.name)))
      onTab(d.id)
      setEditing(d.id)
      onChanged()
    } catch (err) {
      toast(`Couldn’t add a dashboard. Nothing changed. The server said: ${createErrorMessage(err)}`, 'error')
    } finally {
      setCreating(false)
    }
  }

  const rename = async (d: ProjectGroupDashboard, name: string | null): Promise<void> => {
    setEditing(null)
    focusTab(d.id)
    const trimmed = name?.trim() ?? ''
    if (!trimmed || trimmed === d.name) return
    try {
      await renameProjectGroupDashboard(primaryScope(), show.id, d.id, trimmed)
      onChanged()
    } catch (err) {
      toast(`Couldn’t rename “${d.name}”. It keeps its name. The server said: ${createErrorMessage(err)}`, 'error')
    }
  }

  const remove = async (d: ProjectGroupDashboard): Promise<void> => {
    setConfirmDelete(null)
    try {
      await deleteProjectGroupDashboard(primaryScope(), show.id, d.id)
      // Any tab can be deleted from its menu: the open one stays open
      // unless it was the one deleted.
      const wasOpen = activeTab === d.id
      if (wasOpen) onTab(null)
      onChanged()
      toast(`Deleted the dashboard “${d.name}”.`, 'success')
      const stillOpen = wasOpen ? null : dashboards.find((x) => x.id === activeTab)
      focusTab(stillOpen?.id ?? dashboards.find((x) => x.id !== d.id)?.id ?? null)
    } catch (err) {
      const { code } = daemonErrorInfo(err)
      toast(
        code === 'last_dashboard'
          ? 'Couldn’t delete it: a project keeps at least one dashboard.'
          : `Couldn’t delete “${d.name}”. Nothing changed. The server said: ${createErrorMessage(err)}`,
        'error',
      )
    }
  }

  // Arrow keys move between tabs (the tablist pattern).
  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return
    const tabs = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]'))
    const at = tabs.indexOf(document.activeElement as HTMLElement)
    if (at < 0) return
    e.preventDefault()
    const next = tabs[(at + (e.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length]
    next.focus()
  }

  const active = dashboards.find((d) => d.id === activeTab) ?? null

  // A dashboard's menu: Rename, then Delete… (owners and admins). It is the
  // app's shared right-click menu, so no button sits beside a tab and
  // switching tabs never moves one. From the keyboard it opens under the
  // tab (`at`); from a right-click, at the pointer.
  const openTabMenu = async (d: ProjectGroupDashboard, at?: HTMLElement): Promise<void> => {
    const items: ContextMenuItemDef[] = [
      { id: 'rename', label: 'Rename', shortcut: 'F2' },
      { id: 'sep', label: '', type: 'separator' },
      {
        id: 'delete',
        label: 'Delete…',
        danger: true,
        ...(lastOne ? { enabled: false, hint: 'A project keeps at least one dashboard.' } : {}),
      },
    ]
    const r = at?.getBoundingClientRect()
    const picked = await (r ? useContextMenuStore.getState().show(r.left, r.bottom, items) : showContextMenu(items))
    if (picked === 'rename') setEditing(d.id)
    else if (picked === 'delete' && !lastOne) setConfirmDelete(d)
  }

  // Right-click, Shift+F10, the context-menu key and F2 on a dashboard's
  // tab (or on the picker, for the open one). A Member gets none of them,
  // and never the browser's own menu.
  const menuProps = (d: ProjectGroupDashboard | null): TabMenuProps => ({
    onContextMenu: (e) => {
      e.preventDefault()
      e.stopPropagation()
      // The context-menu key can also fire a contextmenu after its keydown
      // opened the menu under the tab: keep that one, not one at the pointer.
      if (useContextMenuStore.getState().isOpen) return
      if (canEdit && d) void openTabMenu(d)
    },
    onKeyDown: (e) => {
      if (!canEdit || !d) return
      if (e.key === 'ContextMenu' || (e.key === 'F10' && e.shiftKey)) {
        e.preventDefault()
        e.stopPropagation()
        void openTabMenu(d, e.currentTarget)
      } else if (e.key === 'F2') {
        e.preventDefault()
        setEditing(d.id)
      }
    },
  })

  const plus = canEdit ? (
    <div className="flex items-center pl-1 flex-shrink-0">
      <Button
        ref={plusRef}
        variant="quiet"
        className="!border-transparent !px-1.5 flex items-center"
        aria-label="New dashboard"
        title="New dashboard"
        disabled={creating}
        onClick={() => void create()}
        data-new-dashboard=""
      >
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-3.5 h-3.5 block">
          <path d="M8 3.5v9M3.5 8h9" />
        </svg>
      </Button>
    </div>
  ) : null

  return (
    <div ref={roomRef} className="relative flex-1 min-w-0 self-stretch flex items-stretch" data-project-tabs-room="">
      {/* A hidden copy of the full row, measured to decide tabs or picker. */}
      <div
        ref={measureRef}
        aria-hidden="true"
        className="absolute left-0 top-0 h-0 overflow-hidden invisible pointer-events-none flex whitespace-nowrap"
        style={{ width: 0 }}
      >
        <div className="flex">
          {dashboards.map((d) => (
            <span key={d.id} className={tabClass(false)}>
              {d.name}
            </span>
          ))}
          {/* The +, the divider and Feedback. */}
          {canEdit && <span className="inline-block w-8 flex-shrink-0" />}
          <span className="inline-block w-[17px] flex-shrink-0" />
          <span className={tabClass(false)}>
            <span className="inline-block w-[13px] mr-1.5" />
            Feedback
          </span>
        </div>
      </div>

      {fits ? (
        <div
          role="tablist"
          aria-label="Dashboards"
          data-project-tabs=""
          className="flex-1 min-w-0 flex items-stretch overflow-hidden"
          onKeyDown={onKeyDown}
        >
          {dashboards.map((d) => {
            const isActive = activeTab === d.id
            return (
              <div key={d.id} role="none" className="flex items-stretch flex-shrink-0" data-dashboard-tab={d.id}>
                {editing === d.id ? (
                  <RenameField dashboard={d} onDone={(name) => void rename(d, name)} />
                ) : (
                  <button
                    type="button"
                    role="tab"
                    aria-selected={isActive}
                    tabIndex={isActive ? 0 : -1}
                    data-k2-focus=""
                    aria-haspopup={canEdit ? 'menu' : undefined}
                    className={tabClass(isActive)}
                    onClick={() => onTab(d.id)}
                    onDoubleClick={() => canEdit && setEditing(d.id)}
                    {...menuProps(d)}
                    title={canEdit ? `${d.name} · Right-click to rename or delete` : d.name}
                  >
                    {d.name}
                  </button>
                )}
              </div>
            )
          })}
          {plus}
          <span aria-hidden="true" className="self-center h-5 w-px mx-2 flex-shrink-0 bg-[var(--color-border)]" />
          <div role="none" className="flex items-stretch flex-shrink-0">
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === FEEDBACK_TAB}
              tabIndex={activeTab === FEEDBACK_TAB ? 0 : -1}
              data-k2-focus=""
              className={tabClass(activeTab === FEEDBACK_TAB)}
              onClick={() => onTab(FEEDBACK_TAB)}
              title="Tickets this project’s agents filed"
              data-feedback-tab=""
            >
              <TicketsIcon size={13} className="flex-shrink-0 mr-1.5" />
              Feedback
            </button>
          </div>
        </div>
      ) : (
        <div role="group" aria-label="Dashboards" data-project-tabs="" className="flex-1 min-w-0 flex items-stretch">
          {active && editing === active.id ? (
            <RenameField dashboard={active} onDone={(name) => void rename(active, name)} />
          ) : (
            <TabPicker dashboards={dashboards} activeTab={activeTab} onTab={onTab} menuProps={menuProps(active)} />
          )}
          {plus}
        </div>
      )}

      {confirmDelete && (
        <ConfirmSheet
          testId="delete-dashboard-confirm"
          title={`Delete the dashboard “${confirmDelete.name}”?`}
          message="Its tab and how it’s laid out go away. The agents and their files stay as they are."
          confirmLabel="Delete dashboard"
          destructive
          onCancel={() => {
            setConfirmDelete(null)
            focusTab(confirmDelete.id)
          }}
          onConfirm={() => void remove(confirmDelete)}
        />
      )}
    </div>
  )
}

// ── The bar ───────────────────────────────────────────────────────────────

const CHAT_ICON = (
  <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" className="block flex-shrink-0">
    <path d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z" />
  </svg>
)

export default function ProjectBar({
  show,
  dashboards,
  activeTab,
  onTab,
  onChanged,
  chatFolded = false,
  onOpenChat,
}: {
  show: ProjectGroupShow
  dashboards: ProjectGroupDashboard[]
  activeTab: string | null
  onTab: (id: string | null) => void
  /** Refetch the project (a change made from the bar). */
  onChanged: () => void
  /** The page set the open chat aside for now (a narrow Feedback board):
   *  the Chat button shows, and `onOpenChat` brings it back. */
  chatFolded?: boolean
  onOpenChat?: () => void
}): React.JSX.Element {
  const role = useViewerRole()
  const caps = projectSettingsCaps(role)
  const viewerWindow = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const rail = useProjectGroupsStore((s) => s.navCollapsed)
  const chatOpen = useProjectGroupsStore((s) => !s.chatCollapsed) && !chatFolded
  const unread = useUnreadEntry('project', show.id)
  const unreadCount = unread && !unread.muted ? unread.unread + unread.waiting : 0
  const barRef = useRef<HTMLDivElement>(null)
  const barWidth = useBarWidth(barRef)
  const narrow = barWidth > 0 && barWidth < COMPACT_BAR_PX
  const tiny = barWidth > 0 && barWidth < TINY_BAR_PX

  const onDashboard = activeTab !== FEEDBACK_TAB && dashboards.some((d) => d.id === activeTab)

  return (
    <div
      ref={barRef}
      data-project-bar=""
      className="flex items-center gap-4 px-4 border-b border-[var(--color-border)] flex-shrink-0 min-w-0"
      style={{ height: PROJECT_BAR_HEIGHT_PX }}
    >
      <Identity show={show} switcher={rail || narrow} tiny={tiny} />

      <DashboardTabs
        show={show}
        dashboards={dashboards}
        activeTab={activeTab}
        onTab={onTab}
        canEdit={caps.canEditDashboards}
        compact={narrow}
        onChanged={onChanged}
      />

      <div className="flex items-center gap-1.5 flex-shrink-0" data-project-actions="">
        {/* §6.8.4 — re-tiles the open dashboard; owners and admins (the
            daemon saves layouts for them only) in a window that may edit. */}
        {onDashboard && caps.canEditDashboards && !viewerWindow && <DashboardPresetsMenu showWord={!narrow} />}
        <Button
          variant="quiet"
          className="!px-0 w-7 flex items-center justify-center"
          aria-label="Project settings"
          title="Project settings"
          onClick={(e) => openProjectSettings(show.id, e.currentTarget)}
          data-project-settings-button=""
        >
          <SettingsIcon size={14} className="block" />
        </Button>
        {/* The chat opens from here; while it's open this button steps
            aside and the chat's own × (same corner) closes it. */}
        {!chatOpen && (
          <Button
            variant="quiet"
            className="relative flex items-center gap-1.5"
            aria-label={narrow ? 'Open the project chat' : undefined}
            title="Open the project chat"
            onClick={() => (chatFolded && onOpenChat ? onOpenChat() : useProjectGroupsStore.getState().setChatCollapsed(false))}
            data-project-chat-button=""
          >
            {CHAT_ICON}
            {!narrow && <span>Chat</span>}
            {unreadCount > 0 && (
              <span
                data-project-chat-unread=""
                aria-label={`${unreadCount} unread`}
                className="min-w-[16px] h-4 px-1 inline-flex items-center justify-center rounded-[var(--radius-badge)] bg-[var(--color-accent-fill)] text-[var(--color-on-accent)] type-badge tabular-nums"
              >
                {unreadCount > 99 ? '99+' : unreadCount}
              </span>
            )}
          </Button>
        )}
      </div>
    </div>
  )
}
