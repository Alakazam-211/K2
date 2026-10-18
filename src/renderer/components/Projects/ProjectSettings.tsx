// Projects V1 P8 (prd-projects-v1 §6.5) — the per-project Settings
// surface. RELOCATION directive (Rosson, 2026-07-05 live review): this
// lives INSIDE the core Settings area as its own "Projects" section
// (Settings.tsx, id 'project-groups' — NOT the legacy 'projects' id,
// which is the Workspaces section), so projects flow exactly like
// workspaces. Deep-links (the nav right-click "Project Settings") open
// Settings at this section with a project preselected via
// useSettingsStore.initialProjectGroupId.
//
// Master-detail: left column = the selectable PROJECT list (search +
// ArrowUp/ArrowDown/Enter + scroll-into-view, Pinned first like the
// nav); right = the selected project.
//
// 0.46.1 LAYOUT (Rosson, 2026-10-18; eval-project-settings-v1.md): no
// tabs and no long single scroll with a nested tab strip. A fixed
// header (picture, name, "Open project"), one banner when something is
// read-only, then TWO HALVES that scroll on their own:
//   - left  = General (name, picture, color, pinned, delete) and
//             Dashboards. The dashboard selector is a sticky bar inside
//             the left half (top AND bottom sticky), so it never scrolls
//             out of view;
//   - right = Agents (members, point of contact, add agent).
// Below HALVES_MIN_WIDTH the halves stack into one column.
//
// Permissions (0.46.1, Rosson): from the login's ROLE as the daemon
// reports it (auth/whoami → fetchViewerRole), never the window's
// eye/pencil switch. Name, pin and agents: any login. Picture, color,
// dashboards and deleting the project: owner or admin (the daemon
// enforces the same floors). Locked rows still show their value.
//
// Dashboards are changed one pane at a time through the daemon
// (dashboard/add-pane + remove-pane), never by saving a whole layout
// read from this client's cache.
//
// Data (0.46.1 — eval-project-settings-v1 P3/P4):
//   - Live: refetch keys on `selectGroupRevision(id)` — only a structural
//     change to THIS project (or a frame naming none, from an older
//     daemon). Chat lines and other projects' changes cost nothing.
//   - Switching never blanks: a project seen before paints at once from
//     the store's show cache and revalidates; an unseen one keeps the
//     previous project on screen (dimmed, inert) until its show lands.
//   - Name / pin / point of contact are optimistic: applied at once to
//     this view and the store's list, confirmed from the daemon's answer,
//     rolled back with an error when it refuses.
//   - The picture comes from the shared icon cache (`useGroupIcon`), keyed
//     by the daemon's `iconKey`, and is requested in parallel with show.

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import {
  getCachedShow,
  selectGroupRevision,
  setCachedShow,
  useProjectGroupsStore,
} from '@/stores/project-groups'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { usePageViewStore } from '@/stores/page-view'
import {
  addDashboardPane,
  addProjectGroupMember,
  createErrorMessage,
  createProjectGroupDashboard,
  daemonErrorInfo,
  deleteProjectGroup,
  deleteProjectGroupDashboard,
  fetchProjectGroupResources,
  fetchProjectGroupShow,
  normalizeHexColor,
  partitionPinned,
  pinProjectGroup,
  removeDashboardPane,
  removeProjectGroupMember,
  renameProjectGroup,
  renameProjectGroupDashboard,
  reorderProjectGroupDashboards,
  setProjectGroupColor,
  setProjectGroupIcon,
  setProjectGroupPoc,
  type ProjectGroup,
  type ProjectGroupDashboard,
  type ProjectGroupHtmlDoc,
  type ProjectGroupShow,
} from './projects-api'
import { onWorkspaceResourcesChanged } from '@/stores/session-events'
import { pickIconImage } from '@/lib/pick-remote-image'
import ProjectGroupAvatar, { GROUP_AVATAR_COLORS, groupAvatarColor, useGroupIcon } from './ProjectGroupAvatar'
import { loadGroupIcon, setCachedGroupIcon } from './group-icon-cache'
import IconCropDialog from '@/components/Settings/IconCropDialog'
import {
  addableWorkspaces,
  dashboardShowsDoc,
  filterGroupsByQuery,
  projectSettingsCaps,
  readOnlyBannerText,
  removeMemberBlockedReason,
  type ProjectRole,
  type ProjectSettingsCaps,
} from './project-settings'
import { moveDashboardId, orderedDashboards } from './project-tabs'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { Button, Callout, Input, Toggle, cx } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { primaryScope } from '@/kessel/server-scope'

/** Below this width (px) of the detail body the two halves stack. */
export const HALVES_MIN_WIDTH = 720

/** Uniform section header (the ProjectsSection h3 idiom). */
function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
      {children}
    </h3>
  )
}

/** One labelled row: label column + control (wraps at narrow widths). */
function Row({
  label,
  settingId,
  children,
}: {
  label: string
  settingId?: string
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div
      className="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-2 border-b border-[var(--color-border)] last:border-b-0"
      data-settings-id={settingId}
    >
      <span className="w-24 flex-shrink-0 text-xs text-[var(--color-text-secondary)]">{label}</span>
      <div className="flex-1 min-w-0 flex items-center gap-2 flex-wrap">{children}</div>
    </div>
  )
}

function errorMessage(err: unknown): string {
  const { hint } = daemonErrorInfo(err)
  if (hint) return hint
  return err instanceof Error ? err.message : String(err)
}

/** Width of an element, live (ResizeObserver). `fallback` until the
 *  first measurement, or forever where ResizeObserver doesn't exist. */
function useElementWidth(fallback: number): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(fallback)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const measure = (): void => {
      const w = el.getBoundingClientRect().width
      if (w > 0) setWidth(w)
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, width]
}

// ── Name (always-visible field; Enter/blur saves) ────────────────────────

function NameField({
  value,
  label,
  readOnly,
  settingId,
  onSave,
}: {
  value: string
  label: string
  readOnly: boolean
  settingId?: string
  onSave: (name: string) => Promise<void>
}): React.JSX.Element {
  const [draft, setDraft] = useState(value)
  const [focused, setFocused] = useState(false)
  const [status, setStatus] = useState<'idle' | 'saving' | 'saved'>('idle')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    // A rename elsewhere wins over an unfocused field; a field being
    // typed in keeps its draft.
    if (!focused) setDraft(value)
  }, [value, focused])

  const commit = async (): Promise<void> => {
    const name = draft.trim()
    if (!name || name === value) {
      setDraft(value)
      setError(null)
      return
    }
    setStatus('saving')
    try {
      await onSave(name)
      setError(null)
      setStatus('saved')
    } catch (err) {
      setError(createErrorMessage(err))
      setStatus('idle')
    }
  }

  return (
    <Row label="Name" settingId={settingId}>
      <Input
        type="text"
        aria-label={`${label} name`}
        value={draft}
        readOnly={readOnly}
        disabled={status === 'saving'}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false)
          if (!readOnly) void commit()
        }}
        onChange={(e) => {
          setDraft(e.target.value)
          setStatus('idle')
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
          else if (e.key === 'Escape') {
            // Local Esc: revert WITHOUT closing Settings.
            e.stopPropagation()
            setDraft(value)
            setError(null)
          }
        }}
        className="flex-1 min-w-[8rem] text-xs"
      />
      <span className="w-12 text-[10px] text-[var(--color-text-muted)]" aria-live="polite">
        {status === 'saving' ? 'Saving…' : status === 'saved' ? 'Saved' : ''}
      </span>
      {error && <p className="basis-full text-[10px] text-[var(--color-status-error-soft)]">{error}</p>}
    </Row>
  )
}

// ── Dashboards (left half; a sticky selector, no tab strip) ──────────────

/** "New dashboard": a button that becomes a name field in place. */
function NewDashboardControl({
  groupId,
  onCreated,
}: {
  groupId: string
  onCreated: (dashboard: ProjectGroupDashboard) => void
}): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (): Promise<void> => {
    const trimmed = name.trim()
    if (!trimmed || busy) return
    setBusy(true)
    try {
      const dashboard = await createProjectGroupDashboard(primaryScope(), groupId, trimmed)
      setEditing(false)
      setName('')
      setError(null)
      onCreated(dashboard)
    } catch (err) {
      setError(createErrorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  if (!editing) {
    return (
      <Button variant="quiet" onClick={() => setEditing(true)}>
        New dashboard
      </Button>
    )
  }
  return (
    <span className="flex items-center gap-1.5" title={error ?? undefined}>
      <Input
        autoFocus
        type="text"
        aria-label="New dashboard name"
        value={name}
        disabled={busy}
        onChange={(e) => {
          setName(e.target.value)
          setError(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void submit()
          else if (e.key === 'Escape') {
            e.stopPropagation()
            setEditing(false)
            setName('')
            setError(null)
          }
        }}
        placeholder="Dashboard name"
        className={cx('w-32', error && 'border-[var(--color-status-error-soft)]')}
      />
      <Button variant="accent" disabled={busy || name.trim().length === 0} onClick={() => void submit()}>
        {busy ? 'Adding…' : 'Add'}
      </Button>
    </span>
  )
}

function DashboardsSection({
  detail,
  caps,
  docs,
  docsError,
}: {
  detail: ProjectGroupShow
  caps: ProjectSettingsCaps
  docs: ProjectGroupHtmlDoc[] | null
  docsError: string | null
}): React.JSX.Element {
  const readOnly = !caps.canEditDashboards
  const dashboards = useMemo(() => orderedDashboards(detail.dashboards), [detail.dashboards])
  const orderedIds = useMemo(() => dashboards.map((d) => d.id), [dashboards])

  // Selection heals to the first dashboard when the selected one was
  // deleted elsewhere.
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const active = dashboards.find((d) => d.id === selectedId) ?? dashboards[0] ?? null
  const activeIndex = active ? orderedIds.indexOf(active.id) : -1
  const lastOne = dashboards.length === 1

  // The newest layout this client has seen per dashboard: the add/remove
  // route returns the dashboard, so the button flips at once; a newer
  // `show` (higher revision) wins.
  const [seen, setSeen] = useState<Record<string, ProjectGroupDashboard>>({})
  const layoutOf = (d: ProjectGroupDashboard): string => {
    const mine = seen[d.id]
    return mine && mine.revision > d.revision ? mine.layoutJson : d.layoutJson
  }

  const [busy, setBusy] = useState(false)
  const [docBusy, setDocBusy] = useState<string | null>(null)

  const reorder = useCallback(
    async (dashboardId: string, direction: -1 | 1): Promise<void> => {
      const order = moveDashboardId(orderedIds, dashboardId, direction)
      if (order === null || busy) return
      setBusy(true)
      try {
        await reorderProjectGroupDashboards(primaryScope(), detail.id, order)
      } catch (err) {
        useToastStore.getState().addToast(`Couldn't move the dashboard: ${errorMessage(err)}`, 'error')
      } finally {
        setBusy(false)
      }
    },
    [detail.id, orderedIds, busy],
  )

  const deleteDashboard = useCallback(
    async (dashboard: ProjectGroupDashboard): Promise<void> => {
      const confirmed = await useConfirmDialogStore.getState().confirm({
        title: `Delete dashboard "${dashboard.name}"?`,
        message: 'This removes the dashboard and how it is laid out. Agents and files are not touched.',
        confirmLabel: 'Delete dashboard',
        destructive: true,
      })
      if (!confirmed) return
      try {
        await deleteProjectGroupDashboard(primaryScope(), detail.id, dashboard.id)
        setSelectedId(null)
      } catch (err) {
        const { code } = daemonErrorInfo(err)
        useToastStore
          .getState()
          .addToast(
            code === 'last_dashboard'
              ? 'A project keeps at least one dashboard.'
              : `Couldn't delete the dashboard: ${errorMessage(err)}`,
            'error',
          )
      }
    },
    [detail.id],
  )

  const toggleDoc = useCallback(
    async (doc: ProjectGroupHtmlDoc, dashboard: ProjectGroupDashboard, shown: boolean): Promise<void> => {
      const key = `${doc.workspaceId}:${doc.filePath}`
      setDocBusy(key)
      const pane = { kind: 'htmlDoc' as const, workspaceId: doc.workspaceId, filePath: doc.filePath }
      try {
        const result = shown
          ? await removeDashboardPane(primaryScope(), detail.id, dashboard.id, pane)
          : await addDashboardPane(primaryScope(), detail.id, dashboard.id, pane)
        setSeen((s) => ({ ...s, [result.id]: result }))
      } catch (err) {
        useToastStore
          .getState()
          .addToast(
            `Couldn't ${shown ? 'remove' : 'add'} ${doc.fileName}: ${errorMessage(err)}`,
            'error',
          )
      } finally {
        setDocBusy(null)
      }
    },
    [detail.id],
  )

  return (
    <>
      {/* The selector bar: a DIRECT child of the left half, sticky at its
          top AND bottom, so it stays in view however far the half is
          scrolled (above it General, below it the dashboard). */}
      <div
        data-testid="dashboard-selector"
        role="group"
        aria-label="Dashboard picker"
        data-settings-id="project-groups.dashboards"
        className="sticky top-0 bottom-0 z-10 bg-[var(--color-bg)] border-y border-[var(--color-border)] py-2 flex flex-wrap items-center gap-2"
      >
        <SectionTitle>Dashboards</SectionTitle>
        <div className="flex-1 min-w-[9rem]">
          <SettingDropdown
            fullWidth
            menuAlign="left"
            ariaLabel="Dashboard"
            value={active?.id ?? ''}
            disabled={dashboards.length === 0}
            onChange={(value) => setSelectedId(value)}
            options={
              dashboards.length === 0
                ? [{ value: '', label: 'No dashboards' }]
                : dashboards.map((d) => ({ value: d.id, label: d.name }))
            }
          />
        </div>
        {!readOnly && dashboards.length > 1 && active && (
          <span className="flex items-center gap-1">
            <Button
              variant="quiet"
              aria-label="Move dashboard earlier"
              title="Move earlier"
              disabled={busy || activeIndex <= 0}
              onClick={() => void reorder(active.id, -1)}
            >
              ‹
            </Button>
            <Button
              variant="quiet"
              aria-label="Move dashboard later"
              title="Move later"
              disabled={busy || activeIndex >= dashboards.length - 1}
              onClick={() => void reorder(active.id, 1)}
            >
              ›
            </Button>
          </span>
        )}
        {!readOnly && <NewDashboardControl groupId={detail.id} onCreated={(d) => setSelectedId(d.id)} />}
      </div>

      <section aria-label="Dashboards">
        {active && (
          <div className="space-y-1">
            <NameField
              key={active.id}
              label="Dashboard"
              value={active.name}
              readOnly={readOnly}
              settingId="project-groups.dashboard-name"
              onSave={async (name) => {
                await renameProjectGroupDashboard(primaryScope(), detail.id, active.id, name)
              }}
            />

            <div className="pt-1 space-y-1.5" data-settings-id="project-groups.dashboard-files">
              <p className="text-xs text-[var(--color-text-secondary)]">Files</p>
              {docsError ? (
                <p className="text-[11px] text-[var(--color-status-error-soft)]">
                  Couldn&apos;t load this project&apos;s files: {docsError}
                </p>
              ) : docs === null ? (
                <p className="text-[11px] text-[var(--color-text-muted)]">Loading files…</p>
              ) : docs.length === 0 ? (
                <p className="text-[11px] text-[var(--color-text-muted)]">
                  No files yet. Add files from an agent&apos;s Files tree.
                </p>
              ) : (
                <ul className="divide-y divide-[var(--color-border)] border-y border-[var(--color-border)]">
                  {docs.map((doc) => {
                    const key = `${doc.workspaceId}:${doc.filePath}`
                    const html = /\.html?$/i.test(doc.filePath)
                    const shown = html && dashboardShowsDoc(layoutOf(active), detail.pocWorkspaceId, doc)
                    return (
                      <li key={key} className="flex items-center gap-2 py-1.5 min-w-0">
                        <div className="flex-1 min-w-0">
                          <p className="text-xs text-[var(--color-text-primary)] truncate">
                            {doc.fileName}
                            <span className="ml-2 text-[10px] text-[var(--color-text-muted)]">
                              {doc.agentName ?? doc.workspaceName ?? ''}
                            </span>
                            {doc.missing ? (
                              <span className="ml-2 text-[10px] text-[var(--color-status-warn-soft)]">missing</span>
                            ) : null}
                          </p>
                          <p className="text-[10px] text-[var(--color-text-muted)] truncate" title={doc.filePath}>
                            {html ? doc.filePath : 'Only HTML files can show on a dashboard for now'}
                          </p>
                        </div>
                        {html && !readOnly && (
                          <Button
                            variant={shown ? 'quiet' : 'default'}
                            disabled={docBusy === key}
                            onClick={() => void toggleDoc(doc, active, shown)}
                          >
                            {shown ? `Remove from ${active.name}` : `Add to ${active.name}`}
                          </Button>
                        )}
                        {html && readOnly && shown && (
                          <span className="text-[10px] text-[var(--color-text-muted)]">On {active.name}</span>
                        )}
                      </li>
                    )
                  })}
                </ul>
              )}
            </div>

            {!readOnly && (
              <div className="flex items-center gap-3 pt-2">
                <p className="flex-1 text-[10px] text-[var(--color-text-muted)]">
                  {lastOne
                    ? 'A project keeps at least one dashboard.'
                    : 'Deleting removes this dashboard only. Agents and files are not touched.'}
                </p>
                <Button
                  variant="quiet"
                  disabled={lastOne}
                  onClick={() => void deleteDashboard(active)}
                  className="text-[var(--color-status-error-soft)]"
                >
                  Delete dashboard
                </Button>
              </div>
            )}
          </div>
        )}
      </section>
    </>
  )
}

// ── Agents (right half) ──────────────────────────────────────────────────

function AgentsSection({
  detail,
  caps,
  edit,
}: {
  detail: ProjectGroupShow
  caps: ProjectSettingsCaps
  edit: GroupEdit
}): React.JSX.Element {
  const readOnly = !caps.canEdit
  const registered = useProjectsStore((s) => s.projects)
  const [query, setQuery] = useState('')
  const [highlight, setHighlight] = useState(0)
  const [memberBusy, setMemberBusy] = useState<string | null>(null)
  const memberIds = useMemo(() => detail.members.map((m) => m.workspaceId), [detail.members])
  const candidates = useMemo(
    () => addableWorkspaces(registered, memberIds, query),
    [registered, memberIds, query],
  )
  useEffect(() => setHighlight(0), [query])

  const addMember = useCallback(
    async (workspaceId: string): Promise<void> => {
      setMemberBusy(workspaceId)
      try {
        await addProjectGroupMember(primaryScope(), detail.id, workspaceId)
        setQuery('')
      } catch (err) {
        useToastStore.getState().addToast(`Couldn't add the agent: ${errorMessage(err)}`, 'error')
      } finally {
        setMemberBusy(null)
      }
    },
    [detail.id],
  )

  const removeMember = useCallback(
    async (workspaceId: string, displayName: string): Promise<void> => {
      setMemberBusy(workspaceId)
      try {
        await removeProjectGroupMember(primaryScope(), detail.id, workspaceId)
      } catch (err) {
        useToastStore
          .getState()
          .addToast(`Couldn't remove ${displayName}: ${errorMessage(err)}`, 'error')
      } finally {
        setMemberBusy(null)
      }
    },
    [detail.id],
  )

  const setPoc = useCallback(
    async (workspaceId: string, displayName: string): Promise<void> => {
      setMemberBusy(workspaceId)
      try {
        await edit({ pocWorkspaceId: workspaceId }, () =>
          setProjectGroupPoc(primaryScope(), detail.id, workspaceId),
        )
        useToastStore
          .getState()
          .addToast(`${displayName} is now the point of contact and gets the project chat.`, 'success')
      } catch (err) {
        useToastStore
          .getState()
          .addToast(`Couldn't change the point of contact: ${errorMessage(err)}`, 'error')
      } finally {
        setMemberBusy(null)
      }
    },
    [detail.id, edit],
  )

  return (
    <section aria-label="Agents" className="space-y-3" data-settings-id="project-groups.agents">
      <div className="space-y-1">
        <SectionTitle>Agents</SectionTitle>
        <p className="text-[10px] text-[var(--color-text-muted)]">
          The point of contact gets every message posted in this project&apos;s chat.
        </p>
      </div>
      <ul className="divide-y divide-[var(--color-border)] border-y border-[var(--color-border)]">
        {detail.members.map((m) => {
          const isPoc = m.workspaceId === detail.pocWorkspaceId
          const blocked = removeMemberBlockedReason(m.workspaceId, detail.pocWorkspaceId)
          const display = m.agentName ?? m.name ?? m.workspaceId.slice(0, 8)
          return (
            <li key={m.workspaceId} className="flex flex-wrap items-center gap-2 py-2 min-w-0" data-testid="project-agent-row">
              <div className="flex-1 min-w-[8rem]">
                <p className="flex items-center gap-2 min-w-0">
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{display}</span>
                  {isPoc && (
                    <span className="text-[9px] font-semibold px-1.5 py-0.5 bg-[color-mix(in_oklab,var(--color-accent)_15%,transparent)] text-[var(--color-accent)] flex-shrink-0">
                      Point of contact
                    </span>
                  )}
                </p>
                {m.path && (
                  <p className="text-[10px] text-[var(--color-text-muted)] truncate" title={m.path}>
                    {m.path}
                  </p>
                )}
              </div>
              {!readOnly && !isPoc && (
                <Button
                  variant="quiet"
                  disabled={memberBusy === m.workspaceId}
                  onClick={() => void setPoc(m.workspaceId, display)}
                >
                  Make point of contact
                </Button>
              )}
              {!readOnly && (
                <Button
                  variant="quiet"
                  disabled={blocked !== null || memberBusy === m.workspaceId}
                  title={blocked ?? 'Remove from this project (the agent itself is not touched)'}
                  onClick={() => void removeMember(m.workspaceId, display)}
                  className={blocked === null ? 'text-[var(--color-status-error-soft)]' : undefined}
                >
                  Remove
                </Button>
              )}
            </li>
          )
        })}
        {detail.members.length === 0 && (
          <li className="py-2 text-[11px] text-[var(--color-text-muted)]">
            No agents yet. The first one you add becomes the point of contact.
          </li>
        )}
      </ul>
      {detail.pocWorkspaceId !== null && detail.members.length > 1 && !readOnly && (
        <p className="text-[10px] text-[var(--color-text-muted)]">
          To remove the point of contact, make another agent the point of contact first.
        </p>
      )}

      {!readOnly && (
        <div className="space-y-1.5" data-settings-id="project-groups.add-agent">
          <p className="text-xs text-[var(--color-text-secondary)]">Add agent</p>
          <Input
            type="text"
            aria-label="Search agents to add"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown') {
                e.preventDefault()
                setHighlight((h) => Math.min(h + 1, candidates.length - 1))
              } else if (e.key === 'ArrowUp') {
                e.preventDefault()
                setHighlight((h) => Math.max(h - 1, 0))
              } else if (e.key === 'Enter' && candidates[highlight]) {
                e.preventDefault()
                void addMember(candidates[highlight].id)
              } else if (e.key === 'Escape' && query) {
                e.stopPropagation()
                setQuery('')
              }
            }}
            placeholder="Search agents…"
            className="w-full text-xs"
          />
          <ul className="max-h-56 overflow-y-auto border border-[var(--color-border)]" aria-label="Agents you can add">
            {candidates.map((w, i) => (
              <li key={w.id} data-highlighted={i === highlight || undefined}>
                <button
                  type="button"
                  disabled={memberBusy === w.id}
                  onMouseEnter={() => setHighlight(i)}
                  onClick={() => void addMember(w.id)}
                  className={cx(
                    'w-full flex items-center gap-2 px-2 py-1.5 text-left transition-colors no-drag cursor-pointer disabled:opacity-50',
                    i === highlight
                      ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)]',
                  )}
                >
                  <span className="text-xs truncate">{w.name}</span>
                  <span className="text-[10px] text-[var(--color-text-muted)] truncate flex-1">{w.path}</span>
                  <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0">
                    {memberBusy === w.id ? 'Adding…' : 'Add'}
                  </span>
                </button>
              </li>
            ))}
            {candidates.length === 0 && (
              <li className="px-2 py-2 text-[11px] text-[var(--color-text-muted)]">
                {registered.length === memberIds.length
                  ? 'Every agent on this server is already in this project.'
                  : 'No agents match.'}
              </li>
            )}
          </ul>
        </div>
      )}
    </section>
  )
}

// ── The right panel (selected project's management) ──────────────────────

/** Optimistic group edit: apply `patch` now, run the daemon call, take its
 *  answer, or roll back and rethrow. */
type GroupEdit = (patch: Partial<ProjectGroup>, run: () => Promise<ProjectGroup | void>) => Promise<void>

function ProjectSettingsDetail({
  detail,
  role,
  onDeleted,
  edit,
  docs,
  docsError,
}: {
  detail: ProjectGroupShow
  role: ProjectRole | null
  onDeleted: () => void
  edit: GroupEdit
  /** Workspace Resources (the dashboard "Files" list) — fetched by the
   *  page alongside show (0.46.1); null = not loaded yet. */
  docs: ProjectGroupHtmlDoc[] | null
  docsError: string | null
}): React.JSX.Element {
  const caps = projectSettingsCaps(role)
  const banner = readOnlyBannerText(role)

  // ── §6.7.7 Icon + color ──────────────────────────────────────────────
  // The icon rides its own GET (not in show payloads). 0.46.1: read from
  // the shared cache, keyed by the show view's `iconKey` — fetched once
  // per icon (another client's upload changes the key → one refetch; a
  // rename, pin or chat line never refetches it). null = not known yet.
  const icon = useGroupIcon(detail.id, detail.iconKey)
  const hostKey = activeHostKey(useConnectHostStore((s) => s.activeHost))
  const [, setIconSaves] = useState(0)
  const [iconBusy, setIconBusy] = useState(false)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const iconInputRef = useRef<HTMLInputElement>(null)

  const handleIconFileSelected = (e: React.ChangeEvent<HTMLInputElement>): void => {
    const file = e.target.files?.[0]
    if (!file) return
    const reader = new FileReader()
    reader.onload = () => setCropImage(reader.result as string)
    reader.readAsDataURL(file)
    // Reset input so the same file can be re-selected (workspace idiom).
    e.target.value = ''
  }

  const saveIcon = useCallback(
    async (dataUrl: string | null): Promise<void> => {
      setIconBusy(true)
      try {
        const res = await setProjectGroupIcon(primaryScope(), detail.id, dataUrl)
        // Cache what we just sent under the daemon's new key, so neither
        // this preview nor the nav avatars re-GET it.
        setCachedGroupIcon(hostKey, detail.id, {
          found: dataUrl !== null,
          dataUrl,
          iconKey: dataUrl === null ? null : res.iconKey,
        })
        setIconSaves((n) => n + 1)
      } catch (err) {
        useToastStore.getState().addToast(`Couldn't save the picture: ${errorMessage(err)}`, 'error')
      } finally {
        setIconBusy(false)
      }
    },
    [detail.id, hostKey],
  )

  // Custom-hex color input (§6.7.7 — the palette + a free `#rrggbb`
  // field; Enter applies, invalid shows inline).
  const [customHex, setCustomHex] = useState('')
  const [hexError, setHexError] = useState(false)

  const saveColor = useCallback(
    async (color: string | null): Promise<void> => {
      try {
        await setProjectGroupColor(primaryScope(), detail.id, color)
        // groups-changed refetches the list + show with the new color.
      } catch (err) {
        useToastStore.getState().addToast(`Couldn't change the color: ${errorMessage(err)}`, 'error')
      }
    },
    [detail.id],
  )

  const commitCustomHex = useCallback((): void => {
    const normalized = normalizeHexColor(customHex)
    if (normalized === null) {
      setHexError(customHex.trim().length > 0)
      return
    }
    setHexError(false)
    setCustomHex('')
    void saveColor(normalized)
  }, [customHex, saveColor])

  const deleteProject = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Delete project "${detail.name}"?`,
      message:
        'This removes the project, its dashboards and its whole chat. ' +
        'The agents and their folders are not touched.',
      confirmLabel: 'Delete project',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await deleteProjectGroup(primaryScope(), detail.id)
      onDeleted()
    } catch (err) {
      useToastStore.getState().addToast(`Couldn't delete the project: ${errorMessage(err)}`, 'error')
    }
  }, [detail.id, detail.name, onDeleted])

  const openProject = useCallback((): void => {
    useProjectGroupsStore.getState().selectGroup(detail.id)
    usePageViewStore.getState().setPage('projects')
    useSettingsStore.getState().closeSettings()
  }, [detail.id])

  const memberLabel = (workspaceId: string): string => {
    const m = detail.members.find((x) => x.workspaceId === workspaceId)
    return m?.agentName ?? m?.name ?? workspaceId.slice(0, 8)
  }

  // The group's effective avatar color — canonical when set, else the
  // stable hashed pick (what the nav renders without an override).
  const effectiveColor = detail.color ?? groupAvatarColor(detail.id)
  const swatchClass = (selected: boolean): string =>
    cx(
      'w-5 h-5 flex-shrink-0 no-drag cursor-pointer transition-transform',
      selected
        ? 'outline outline-2 outline-offset-2 outline-[var(--color-accent)]'
        : 'hover:scale-110',
    )

  const [bodyRef, bodyWidth] = useElementWidth(HALVES_MIN_WIDTH)
  const stacked = bodyWidth < HALVES_MIN_WIDTH

  const general = (
    <section aria-label="General" className="space-y-1">
      <SectionTitle>General</SectionTitle>
      <NameField
        label="Project"
        value={detail.name}
        readOnly={!caps.canEdit}
        settingId="project-groups.name"
        onSave={(name) => edit({ name }, () => renameProjectGroup(primaryScope(), detail.id, name))}
      />
      <Row label="Picture" settingId="project-groups.picture">
        <ProjectGroupAvatar
          name={detail.name}
          groupId={detail.id}
          size={48}
          color={detail.color}
          iconUrl={icon?.dataUrl ?? null}
        />
        {caps.canEditLooks && (
          <>
            <Button
              variant="quiet"
              onClick={() =>
                // Host-aware: on a remote host the native input would
                // browse THIS machine's disk — use the remote picker.
                void pickIconImage({
                  clickNativeInput: () => iconInputRef.current?.click(),
                  setCropImage,
                })
              }
              disabled={iconBusy}
            >
              {iconBusy ? 'Working…' : 'Upload'}
            </Button>
            <input
              ref={iconInputRef}
              type="file"
              accept="image/png,image/jpeg,image/svg+xml,image/x-icon"
              className="hidden"
              onChange={handleIconFileSelected}
            />
            <Button
              variant="quiet"
              onClick={() => void saveIcon(null)}
              disabled={iconBusy || !icon?.dataUrl}
              title={icon?.dataUrl ? 'Remove the picture' : 'No picture yet'}
            >
              Remove
            </Button>
          </>
        )}
      </Row>
      <Row label="Color" settingId="project-groups.color">
        {caps.canEditLooks ? (
          <>
            <button
              type="button"
              aria-label="Automatic color"
              aria-pressed={detail.color === null}
              onClick={() => void saveColor(null)}
              title="Automatic: a steady color picked for this project"
              className={cx(swatchClass(detail.color === null), 'text-[9px] font-bold leading-none text-[var(--color-on-accent)]')}
              style={{ backgroundColor: groupAvatarColor(detail.id) }}
            >
              A
            </button>
            {GROUP_AVATAR_COLORS.map((color) => (
              <button
                key={color}
                type="button"
                aria-label={`Color ${color}`}
                aria-pressed={detail.color === color}
                onClick={() => void saveColor(color)}
                className={swatchClass(detail.color === color)}
                style={{ backgroundColor: color }}
              />
            ))}
            <Input
              type="text"
              aria-label="Custom color"
              value={customHex}
              onChange={(e) => {
                setCustomHex(e.target.value)
                setHexError(false)
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitCustomHex()
                else if (e.key === 'Escape') {
                  e.stopPropagation()
                  setCustomHex('')
                  setHexError(false)
                }
              }}
              onBlur={() => commitCustomHex()}
              placeholder="#rrggbb"
              title={hexError ? 'Use #rrggbb' : undefined}
              className={cx('w-20 font-mono', hexError && 'border-[var(--color-status-error-soft)]')}
            />
          </>
        ) : (
          <span className="flex items-center gap-2 text-xs text-[var(--color-text-primary)]">
            <span className="w-5 h-5 flex-shrink-0 inline-block" style={{ backgroundColor: effectiveColor }} />
            {detail.color ?? 'Automatic'}
          </span>
        )}
      </Row>
      <Row label="Show in Pinned" settingId="project-groups.pinned">
        <Toggle
          aria-label="Show in Pinned"
          checked={detail.pinned}
          disabled={!caps.canEdit}
          onChange={(next) => {
            void edit({ pinned: next }, () => pinProjectGroup(primaryScope(), detail.id, next)).catch((err) =>
              useToastStore.getState().addToast(`Couldn't change Pinned: ${errorMessage(err)}`, 'error'),
            )
          }}
        />
        <span className="text-[10px] text-[var(--color-text-muted)]">Everyone on this server sees it pinned.</span>
      </Row>
      <Row label="Delete" settingId="project-groups.delete">
        <span className="flex-1 min-w-[10rem] text-[10px] text-[var(--color-text-muted)]">
          Removes the project, its dashboards and its chat. The agents and their folders stay.
        </span>
        <Button
          variant="danger"
          disabled={!caps.canDelete}
          title={caps.canDelete ? undefined : 'Only the owner or an admin can delete a project'}
          onClick={() => void deleteProject()}
        >
          Delete project
        </Button>
      </Row>
    </section>
  )

  return (
    <>
      {cropImage && (
        <IconCropDialog
          imageDataUrl={cropImage}
          onConfirm={(cropped) => {
            setCropImage(null)
            void saveIcon(cropped)
          }}
          onCancel={() => setCropImage(null)}
        />
      )}
      <div className="flex flex-col h-full min-h-0" data-testid="project-settings-detail">
        {/* ── Header (fixed; never scrolls away) ── */}
        <div className="flex-shrink-0 px-6 pt-5 pb-3 space-y-3 border-b border-[var(--color-border)]">
          <div className="flex items-center gap-3 min-w-0">
            <ProjectGroupAvatar
              name={detail.name}
              groupId={detail.id}
              size={40}
              color={detail.color}
              iconUrl={icon?.dataUrl ?? null}
            />
            <div className="flex-1 min-w-0">
              <h2 className="text-base font-medium text-[var(--color-text-primary)] truncate">{detail.name}</h2>
              <p className="text-[11px] text-[var(--color-text-muted)] truncate">
                {detail.memberCount} {detail.memberCount === 1 ? 'agent' : 'agents'}
                {detail.pocWorkspaceId
                  ? ` · Point of contact: ${memberLabel(detail.pocWorkspaceId)}`
                  : ' · No point of contact yet'}
              </p>
            </div>
            <Button variant="default" onClick={openProject}>
              Open project
            </Button>
          </div>
          {banner && (
            <Callout tone="info" data-testid="project-settings-readonly-banner">
              {banner}
            </Callout>
          )}
        </div>

        {/* ── Two halves: left General + Dashboards, right Agents ── */}
        <div
          ref={bodyRef}
          data-testid="project-settings-halves"
          data-layout={stacked ? 'stacked' : 'split'}
          className={cx(
            'flex-1 min-h-0',
            stacked ? 'overflow-y-auto' : 'grid grid-cols-2 divide-x divide-[var(--color-border)]',
          )}
        >
          <div
            data-testid="project-settings-left"
            className={cx('px-6 pb-6 space-y-6', stacked ? '' : 'overflow-y-auto min-h-0')}
          >
            <div className="pt-4">{general}</div>
            <DashboardsSection detail={detail} caps={caps} docs={docs} docsError={docsError} />
          </div>
          <div
            data-testid="project-settings-right"
            className={cx(
              'px-6 pb-6 pt-4',
              stacked ? 'border-t border-[var(--color-border)]' : 'overflow-y-auto min-h-0',
            )}
          >
            <AgentsSection detail={detail} caps={caps} edit={edit} />
          </div>
        </div>
      </div>
    </>
  )
}

// ── The settings surface (master-detail) ─────────────────────────────────

/** Last Workspace Resources answer per (host, project) — a revisit paints
 *  its list at once instead of "Loading…" (0.46.1). */
const docsCache = new Map<string, ProjectGroupHtmlDoc[]>()

function docsKey(groupId: string): string {
  return `${activeHostKey(useConnectHostStore.getState().activeHost)}:${groupId}`
}

/** Fields with an optimistic edit in flight, per project: they win over
 *  any show answer that lands meanwhile, so a refetch can't snap them
 *  back. `seq` lets a newer edit of the same field outlive an older one. */
type PendingEdits = Map<string, Map<string, { value: unknown; seq: number }>>

function withPending(show: ProjectGroupShow, pending: PendingEdits): ProjectGroupShow {
  const fields = pending.get(show.id)
  if (!fields || fields.size === 0) return show
  const out: Record<string, unknown> = { ...show }
  for (const [k, v] of fields) out[k] = v.value
  return out as unknown as ProjectGroupShow
}

export default function ProjectSettings(): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const groupsError = useProjectGroupsStore((s) => s.groupsError)

  // The signed-in person's role on this server (owner token → owner).
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

  // The section's OWN selection — seeded from the deep-link preselect
  // (initialProjectGroupId — the gear / right-click), else the Projects
  // page's nav selection, else the first group. Switching here never
  // moves the page's nav selection.
  const initialGroupId = useSettingsStore((s) => s.initialProjectGroupId)
  const [selectedId, setSelectedId] = useState<string | null>(
    () => initialGroupId ?? useProjectGroupsStore.getState().selectedGroupId,
  )
  // This project's structural revision: a chat line or another
  // project's change never refetches the panel.
  const revision = useProjectGroupsStore(selectGroupRevision(selectedId))
  const pendingRef = useRef<PendingEdits>(new Map())
  const editSeq = useRef(0)
  const [detail, setDetail] = useState<ProjectGroupShow | null>(() => {
    const cached = selectedId ? getCachedShow(selectedId) : undefined
    return cached ?? null
  })
  const [detailError, setDetailError] = useState<{ id: string; message: string } | null>(null)
  const [docsState, setDocsState] = useState<{
    id: string
    docs: ProjectGroupHtmlDoc[] | null
    error: string | null
  } | null>(null)
  const [reloadNonce, setReloadNonce] = useState(0)

  // A new deep-link while Settings is already open (right-click another
  // project) re-targets the selection (the ProjectsSection
  // initialProjectId idiom).
  useEffect(() => {
    if (initialGroupId) setSelectedId(initialGroupId)
  }, [initialGroupId])

  // The group list may be stale/unfetched when Settings opens before the
  // Projects page ever did — fetch on mount (events keep it live after).
  useEffect(() => {
    void useProjectGroupsStore.getState().fetchGroups()
  }, [])

  // No selection yet, or the selected group vanished (deleted on another
  // client) → fall back to the first group (null when none exist).
  useEffect(() => {
    if (groups === null) return
    if (selectedId === null || !groups.some((g) => g.id === selectedId)) {
      setSelectedId(groups[0]?.id ?? null)
    }
  }, [groups, selectedId])

  // Fetch the selected project's show view on selection change + every
  // structural change to THIS project (selectGroupRevision). The picture
  // is requested in parallel (from the list row's iconKey), not after
  // show lands; a cached one costs nothing.
  useEffect(() => {
    if (selectedId === null) {
      setDetail(null)
      setDetailError(null)
      return
    }
    let cancelled = false
    fetchProjectGroupShow(primaryScope(), selectedId)
      .then((data) => {
        if (cancelled) return
        setCachedShow(data)
        setDetail(withPending(data, pendingRef.current))
        setDetailError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDetailError({ id: selectedId, message: e instanceof Error ? e.message : String(e) })
      })
    const row = useProjectGroupsStore.getState().groups?.find((g) => g.id === selectedId)
    void loadGroupIcon(activeHostKey(useConnectHostStore.getState().activeHost), selectedId, row?.iconKey)
    return () => {
      cancelled = true
    }
  }, [selectedId, revision, reloadNonce])

  // Workspace Resources, fetched in parallel with show (not after the
  // panel mounts) and on resource changes. Same project revision key.
  useEffect(() => {
    if (selectedId === null) return
    let cancelled = false
    const load = (): void => {
      fetchProjectGroupResources(primaryScope(), selectedId)
        .then((d) => {
          if (cancelled) return
          docsCache.set(docsKey(selectedId), d)
          setDocsState({ id: selectedId, docs: d, error: null })
        })
        .catch((e) => {
          if (cancelled) return
          setDocsState({
            id: selectedId,
            docs: docsCache.get(docsKey(selectedId)) ?? null,
            error: errorMessage(e),
          })
        })
    }
    load()
    const off = onWorkspaceResourcesChanged(primaryScope(), () => load())
    return () => {
      cancelled = true
      off()
    }
  }, [selectedId, revision, reloadNonce])

  // What the panel shows: the selected project's show view; else its
  // cached one (painted at once, revalidating); else the PREVIOUS
  // project's, dimmed and inert until the new one lands — never a blank
  // "Loading…" that collapses the panel and its scroll.
  const cachedSelected =
    selectedId !== null && detail?.id !== selectedId ? getCachedShow(selectedId) : undefined
  const shown =
    detail?.id === selectedId
      ? detail
      : cachedSelected
        ? withPending(cachedSelected, pendingRef.current)
        : detail
  const switching = shown !== null && shown.id !== selectedId
  const shownDocs =
    shown === null
      ? null
      : docsState?.id === shown.id
        ? docsState.docs
        : (docsCache.get(docsKey(shown.id)) ?? null)
  const shownDocsError = shown !== null && docsState?.id === shown.id ? docsState.error : null
  const shownRef = useRef(shown)
  shownRef.current = shown

  // Optimistic edit (rename / pin / point of contact): apply now to this
  // view, the show cache and the store's list row; take the daemon's
  // answer; on refusal roll the fields back and rethrow for the caller's
  // error. Fields keep winning over refetches until their edit settles.
  const edit = useCallback(
    async (
      groupId: string,
      patch: Partial<ProjectGroup>,
      run: () => Promise<ProjectGroup | void>,
    ): Promise<void> => {
      const seq = ++editSeq.current
      const keys = Object.keys(patch) as Array<keyof ProjectGroup>
      const base = shownRef.current?.id === groupId ? shownRef.current : getCachedShow(groupId)
      const before: Partial<ProjectGroup> = {}
      for (const k of keys) (before as Record<string, unknown>)[k] = base?.[k]
      let fields = pendingRef.current.get(groupId)
      if (!fields) {
        fields = new Map()
        pendingRef.current.set(groupId, fields)
      }
      for (const k of keys) fields.set(k, { value: patch[k], seq })
      const pending = fields

      const apply = (p: Partial<ProjectGroup>): void => {
        if (Object.keys(p).length === 0) return
        setDetail((d) => (d && d.id === groupId ? { ...d, ...p } : d))
        const cached = getCachedShow(groupId)
        if (cached) setCachedShow({ ...cached, ...p })
        useProjectGroupsStore.getState().patchGroup(groupId, p)
      }
      // Only fields no newer edit has claimed since.
      const settle = (from: Partial<ProjectGroup> | null): Partial<ProjectGroup> => {
        const out: Record<string, unknown> = {}
        for (const k of keys) {
          if (pending.get(k)?.seq !== seq) continue
          pending.delete(k)
          if (from && k in from) out[k] = (from as Record<string, unknown>)[k]
        }
        return out as Partial<ProjectGroup>
      }

      apply(patch)
      let answer: ProjectGroup | void
      try {
        answer = await run()
      } catch (err) {
        apply(settle(before))
        throw err
      }
      apply(settle(answer && typeof answer === 'object' ? answer : null))
    },
    [],
  )

  // ── Left master list: search + keyboard selection, Pinned first ─────
  const [searchQuery, setSearchQuery] = useState('')
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const { pinned: pinnedGroups, unpinned: otherGroups } = useMemo(
    () => partitionPinned(filterGroupsByQuery(groups ?? [], searchQuery)),
    [groups, searchQuery],
  )
  const visibleGroups = useMemo(() => [...pinnedGroups, ...otherGroups], [pinnedGroups, otherGroups])
  useEffect(() => {
    setKeyboardIndex(-1)
  }, [searchQuery])
  useEffect(() => {
    requestAnimationFrame(() => searchInputRef.current?.focus())
  }, [])

  const handleSearchKeyDown = useCallback(
    (e: React.KeyboardEvent): void => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setKeyboardIndex((prev) => Math.min(prev + 1, visibleGroups.length - 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setKeyboardIndex((prev) => Math.max(prev - 1, 0))
      } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < visibleGroups.length) {
        e.preventDefault()
        setSelectedId(visibleGroups[keyboardIndex].id)
      }
    },
    [visibleGroups, keyboardIndex],
  )

  // Scroll the keyboard-highlighted row into view (ProjectsSection).
  useEffect(() => {
    if (keyboardIndex >= 0 && visibleGroups[keyboardIndex]) {
      const el = document.querySelector(
        `[data-settings-group-id="${visibleGroups[keyboardIndex].id}"]`,
      )
      el?.scrollIntoView({ block: 'nearest' })
    }
  }, [keyboardIndex, visibleGroups])

  const retryList = (): void => {
    void useProjectGroupsStore.getState().fetchGroups()
  }

  const groupRow = (g: (typeof visibleGroups)[number]): React.JSX.Element => {
    const isSelected = selectedId === g.id
    const isKeyboardHighlighted = visibleGroups.indexOf(g) === keyboardIndex
    return (
      <div
        key={g.id}
        data-settings-group-id={g.id}
        onClick={() => setSelectedId(g.id)}
        className={cx(
          'flex items-center gap-2 px-2 py-1.5 transition-colors no-drag cursor-pointer select-none',
          isSelected
            ? 'bg-[color-mix(in_oklab,var(--color-accent)_15%,transparent)] text-[var(--color-text-primary)]'
            : isKeyboardHighlighted
              ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
              : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)]',
        )}
      >
        <ProjectGroupAvatar name={g.name} groupId={g.id} size={20} color={g.color} />
        <span className="text-xs truncate flex-1">{g.name}</span>
        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">{g.memberCount}</span>
      </div>
    )
  }

  const listHeading = (label: string): React.JSX.Element => (
    <p className="px-2 pt-2 pb-1 text-[9px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
      {label}
    </p>
  )

  return (
    <div className="flex h-full min-h-0">
      {/* ── Left: the project list ── */}
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col">
        <div className="px-2 py-1.5">
          <Input
            ref={searchInputRef}
            type="text"
            aria-label="Search projects"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleSearchKeyDown}
            placeholder="Search projects…"
            className="w-full text-xs py-1.5"
          />
        </div>
        <div className="flex-1 overflow-y-auto px-1 py-1">
          {pinnedGroups.length > 0 && (
            <>
              {listHeading('Pinned')}
              {pinnedGroups.map(groupRow)}
              {otherGroups.length > 0 && listHeading('All projects')}
            </>
          )}
          {otherGroups.map(groupRow)}
          {groups !== null && visibleGroups.length === 0 && (
            <div className="px-2 py-6 text-center">
              <span className="text-xs text-[var(--color-text-muted)]">
                {searchQuery.trim() ? 'No projects match' : 'No projects'}
              </span>
            </div>
          )}
        </div>
      </div>

      {/* ── Right: the selected project ── */}
      <div className="flex-1 min-h-0 min-w-0">
        {groups === null && groupsError ? (
          <div className="p-6 space-y-2" data-testid="project-settings-list-error">
            <Callout tone="error">Couldn&apos;t load projects: {groupsError}</Callout>
            <Button variant="default" onClick={retryList}>
              Retry
            </Button>
          </div>
        ) : groups !== null && groups.length === 0 ? (
          <p className="p-6 text-xs text-[var(--color-text-muted)]">
            No projects yet. Create one on the Projects page.
          </p>
        ) : selectedId === null || groups === null ? (
          <p className="p-6 text-xs text-[var(--color-text-muted)]">Loading…</p>
        ) : detailError && detailError.id === selectedId ? (
          <div className="p-6 space-y-2" data-testid="project-settings-detail-error">
            <Callout tone="error">Couldn&apos;t load this project: {detailError.message}</Callout>
            <Button variant="default" onClick={() => setReloadNonce((n) => n + 1)}>
              Retry
            </Button>
          </div>
        ) : shown === null ? (
          /* First load only — after that a switch keeps the previous
             project on screen (below) instead of blanking. */
          <p className="p-6 text-xs text-[var(--color-text-muted)]">Loading…</p>
        ) : (
          /* While switching to a project not seen yet, the PREVIOUS one
             stays (dimmed, inert, aria-busy) so the panel never collapses
             or loses its scroll; it can't be edited by mistake. */
          <div
            aria-busy={switching || undefined}
            className={cx('h-full transition-opacity', switching && 'opacity-50 pointer-events-none')}
          >
            <ProjectSettingsDetail
              key={shown.id}
              detail={shown}
              role={role}
              edit={(patch, run) => edit(shown.id, patch, run)}
              docs={shownDocs}
              docsError={shownDocsError}
              onDeleted={() => {
                // Select the first surviving sibling (null when none —
                // the empty state); refetch NOW so the stale row never
                // gets re-selected by the fallback effect.
                const remaining = (useProjectGroupsStore.getState().groups ?? []).filter(
                  (g) => g.id !== selectedId,
                )
                setSelectedId(remaining[0]?.id ?? null)
                void useProjectGroupsStore.getState().fetchGroups()
              }}
            />
          </div>
        )}
      </div>
    </div>
  )
}
