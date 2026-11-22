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
// nav); right = the selected project. The list column, its rows and the
// detail's header are the Workspaces / Agents ones (ProjectsSection), and
// every setting is a row of a `SettingsGroup card`, so the page reads like
// the rest of Settings. With no projects yet (or while the list loads or
// after it failed) the page is one column: a title and one state card.
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
//   - Name / pin / color / point of contact are optimistic: applied at
//     once to this view and the store's list, confirmed from the daemon's
//     answer, rolled back with an error when it refuses.
//   - The picture comes from the shared icon cache (`useGroupIcon`), keyed
//     by the daemon's `iconKey`, and is requested in parallel with show.
//
// Saves say so: a quiet "Saved" by the control after a write lands, and
// "Couldn't save: … Nothing changed; try again." under the row when the
// daemon refuses (SettingControls' SaveNote / SaveError). Deleting a
// project, a dashboard or an agent from the project asks first, in the
// same centred dialog as the rest of Settings (ConfirmSheet).

import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import {
  getCachedShow,
  memberOwnViews,
  selectGroupRevision,
  setCachedShow,
  useProjectGroupsStore,
} from '@/stores/project-groups'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { usePageViewStore } from '@/stores/page-view'
import {
  addDashboardPane,
  addProjectGroupMember,
  createErrorMessage,
  createProjectGroup,
  createProjectGroupDashboard,
  daemonErrorInfo,
  daemonKnowsMemberViews,
  deleteProjectGroup,
  deleteProjectGroupDashboard,
  fetchProjectGroupResources,
  fetchProjectGroupShow,
  normalizeHexColor,
  PROJECT_DEFAULT_VIEWS,
  partitionPinned,
  pinProjectGroup,
  projectDefaultView,
  removeDashboardPane,
  removeProjectGroupMember,
  renameProjectGroup,
  renameProjectGroupDashboard,
  reorderProjectGroupDashboards,
  setProjectGroupColor,
  setProjectGroupDefaultView,
  setProjectGroupIcon,
  setProjectGroupPoc,
  type ProjectDefaultView,
  type ProjectGroup,
  type ProjectGroupDashboard,
  type ProjectGroupHtmlDoc,
  type ProjectGroupMemberInfo,
  type ProjectGroupShow,
} from './projects-api'
import {
  MEMBER_VIEW_NAME,
  memberViewError,
  memberViewSavedText,
  projectDefaultLabel,
  useMemberViews,
  type MemberView,
} from './member-view'
import { ViewIcon } from '@/components/SessionView/ThreadDrawerSelector'
import { onWorkspaceResourcesChanged } from '@/stores/session-events'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { pickIconImage } from '@/lib/pick-remote-image'
import ProjectGroupAvatar, { GROUP_AVATAR_COLORS, groupAvatarColor, useGroupIcon } from './ProjectGroupAvatar'
import { loadGroupIcon, setCachedGroupIcon } from './group-icon-cache'
import IconCropDialog from '@/components/Settings/IconCropDialog'
import {
  addableWorkspaces,
  dashboardShowsDoc,
  defaultViewLockReason,
  filterGroupsByQuery,
  memberViewLockReason,
  projectSettingsCaps,
  readOnlyBannerText,
  removeMemberBlockedReason,
  type ProjectRole,
  type ProjectSettingsCaps,
} from './project-settings'
import { moveDashboardId, orderedDashboards } from './project-tabs'
import {
  AVATAR_ROUND,
  SETTING_FIELD_WIDTH,
  SETTING_LIST_ROW_BAND,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_SERVER_REASON,
  SaveError,
  SaveNote,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
  type SaveState,
} from '@/components/Settings/controls/SettingControls'
import { AccessSheet, ConfirmSheet, DIALOG_FIELD, FailedNote, Field } from '@/components/Settings/sections/AccessDialogs'
import { Button, Callout, Input, OverflowMenu, SearchField, Segmented, Spinner, Surface, Tag, Toggle, cx } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { primaryScope } from '@/kessel/server-scope'
import { clientToCssPx } from '@/lib/app-zoom'
import { SPLIT_PAGE_MIN_PX } from '@/components/Settings/controls/split-page'
import { HALVES_MIN_WIDTH, ScrollEdge, halves, useCssWidth } from '@/components/Settings/controls/halves'
import { PRODUCT_NAME } from '@/lib/brand'

export { HALVES_MIN_WIDTH }

/** Below this page width (CSS px) the project list goes on top of the
 *  selected project instead of beside it: the list's 18rem plus 20rem for
 *  the project's rows (a small window, the app zoomed in). */
export const PROJECTS_SPLIT_MIN_PX = SPLIT_PAGE_MIN_PX

/** What a role refusal (the server's `forbidden`) says: who can do it. */
const ROLE_REFUSED = 'only the owner or an admin of this server can change this'

/** The server's reason for a refusal, in its own words (no prefix). A role
 *  refusal is always the same plain words, whatever the server adds. */
function errorMessage(err: unknown): string {
  const { code, hint } = daemonErrorInfo(err)
  const raw = err instanceof Error ? err.message : String(err)
  if ((err instanceof DaemonHttpError && (err.code === 'forbidden' || (err.status === 403 && !err.code))) || code === 'forbidden' || raw.trim() === 'forbidden') {
    return ROLE_REFUSED
  }
  return hint || raw
}

/** A failed write that isn't a plain save (delete, add, move), in the
 *  order the page's state cards use: what failed and that nothing changed,
 *  in plain words; the server's reason, quietly; then the next step (a
 *  "Try again" button when the caller can retry, else a line). A role
 *  refusal says who can do it instead (trying again won't help). */
function ActionError({
  what,
  reason,
  onRetry,
  next = 'Try again in a moment.',
}: {
  what: string
  reason: string
  onRetry?: () => void
  /** The next step in words, when there is no retry. */
  next?: string
}): React.JSX.Element {
  if (reason === ROLE_REFUSED) {
    return (
      <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">
        {what} Only the owner or an admin of this server can do this, so nothing changed.
      </p>
    )
  }
  const why = reason.trim().replace(/[.\s]+$/, '')
  return (
    <div role="alert" data-action-error="" className="max-w-[75ch]">
      <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed">{what} Nothing changed.</p>
      {why && <p className={SETTING_SERVER_REASON}>The server said: {why}.</p>}
      {onRetry ? (
        <Button variant="quiet" onClick={onRetry} className="mt-2">
          Try again
        </Button>
      ) : (
        <p className={SETTING_ROW_HINT}>{next}</p>
      )}
    </div>
  )
}

/** SaveError, except a role refusal: that says who can change it, and no
 *  "try again" (trying again won't help). */
function RowSaveError({ state }: { state: SaveState }): React.JSX.Element | null {
  if (typeof state === 'object' && state.error === ROLE_REFUSED) {
    return (
      <p role="alert" data-save-error="" className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]">
        Couldn&rsquo;t save: only the owner or an admin of this server can change this. Nothing changed.
      </p>
    )
  }
  return <SaveError state={state} />
}

/** "Saving…" beside a control while a write is in flight. */
function SavingNote({ on }: { on: boolean }): React.JSX.Element | null {
  if (!on) return null
  return (
    <span className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap" aria-live="polite">
      Saving…
    </span>
  )
}

/** A longer wait in words with a spinner, in the control's place or beside
 *  it ("Uploading…", "Deleting…"): readable, unlike a disabled button. */
function BusyNote({ text }: { text: string }): React.JSX.Element {
  return (
    <span role="status" className="inline-flex items-center gap-1.5 type-control text-[var(--color-text-secondary)] whitespace-nowrap">
      <Spinner />
      {text}
    </span>
  )
}

/** Where the dashboard bar is in its scroller. `stickBottom`: the bar may
 *  also stick to the bottom edge (so it is in view however far up the half
 *  is scrolled) only when General and the bar fit the half together: when
 *  they don't (a short window), a bottom-stuck bar would sit over General's
 *  last rows, so it only sticks to the top. `stuck`: the bar is held at an
 *  edge with content passing under it (it draws a hairline on that side). */
function useDashboardBar(
  hostRef: React.RefObject<HTMLDivElement | null>,
  leadRef: React.RefObject<HTMLDivElement | null>,
  layout: string,
): { stickBottom: boolean; stuck: 'top' | 'bottom' | null } {
  const [state, setState] = useState<{ stickBottom: boolean; stuck: 'top' | 'bottom' | null }>({
    stickBottom: true,
    stuck: null,
  })
  useLayoutEffect(() => {
    const host = hostRef.current
    const lead = leadRef.current
    const bar = host?.querySelector<HTMLElement>('[data-testid="dashboard-selector"]')
    if (!host || !lead || !bar) return
    const update = (): void => {
      const h = host.clientHeight
      if (h <= 0) return
      // The bar's own place in the content (right under General). The rects
      // are on-screen px; scrollTop and clientHeight are CSS px.
      const at = clientToCssPx(lead.getBoundingClientRect().bottom - host.getBoundingClientRect().top) + host.scrollTop
      const barH = bar.offsetHeight
      const stickBottom = at + barH <= h
      const y = at - host.scrollTop
      const stuck = y < 0 ? 'top' : stickBottom && y + barH > h ? 'bottom' : null
      setState((s) => (s.stickBottom === stickBottom && s.stuck === stuck ? s : { stickBottom, stuck }))
    }
    update()
    host.addEventListener('scroll', update, { passive: true })
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update)
    ro?.observe(host)
    ro?.observe(lead)
    ro?.observe(bar)
    return () => {
      host.removeEventListener('scroll', update)
      ro?.disconnect()
    }
  }, [hostRef, leadRef, layout])
  return state
}

// ── Name (always-visible field; Enter/blur saves) ────────────────────────

function NameField({
  value,
  label,
  hint,
  readOnly,
  settingId,
  onSave,
}: {
  value: string
  label: string
  hint: string
  readOnly: boolean
  settingId?: string
  onSave: (name: string) => Promise<void>
}): React.JSX.Element {
  const [draft, setDraft] = useState(value)
  const [focused, setFocused] = useState(false)
  const [saving, setSaving] = useState(false)
  const save = useSaveStatus()

  useEffect(() => {
    // A rename elsewhere wins over an unfocused field; a field being
    // typed in keeps its draft.
    if (!focused) setDraft(value)
  }, [value, focused])

  const commit = async (): Promise<void> => {
    const name = draft.trim()
    if (!name || name === value) {
      setDraft(value)
      return
    }
    setSaving(true)
    try {
      await onSave(name)
      save.saved()
    } catch (err) {
      save.failed(new Error(createErrorMessage(err)))
    } finally {
      setSaving(false)
    }
  }

  return (
    <SettingRow label="Name" hint={hint} settingId={settingId} below={belowFor(save.state)}>
      <SavingNote on={saving} />
      <SaveNote state={save.state} />
      <Input
        type="text"
        aria-label={`${label} name`}
        value={draft}
        readOnly={readOnly}
        disabled={saving}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false)
          if (!readOnly) void commit()
        }}
        onChange={(e) => {
          setDraft(e.target.value)
          if (save.state !== 'idle') save.reset()
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
          else if (e.key === 'Escape') {
            // Local Esc: revert WITHOUT closing Settings.
            e.stopPropagation()
            setDraft(value)
            save.reset()
          }
        }}
        className={cx(SETTING_FIELD_WIDTH, 'type-control')}
      />
    </SettingRow>
  )
}

/** A SaveError that is empty when there's nothing to say (SettingRow's
 *  `below` takes a node; null keeps the row tight). */
function belowFor(state: SaveState, extra?: React.ReactNode): React.ReactNode {
  if (typeof state !== 'object' && !extra) return undefined
  return (
    <>
      <RowSaveError state={state} />
      {extra}
    </>
  )
}

// ── Dashboards (left half; a sticky selector, no tab strip) ──────────────

/** "New dashboard": the button and its dialog (name, then Add). */
function NewDashboardDialog({
  groupId,
  onCreated,
  onClose,
}: {
  groupId: string
  onCreated: (dashboard: ProjectGroupDashboard) => void
  onClose: () => void
}): React.JSX.Element {
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const fieldId = useId()

  const submit = async (): Promise<void> => {
    const trimmed = name.trim()
    if (!trimmed || busy) return
    setBusy(true)
    try {
      const dashboard = await createProjectGroupDashboard(primaryScope(), groupId, trimmed)
      onCreated(dashboard)
    } catch (err) {
      setError(createErrorMessage(err))
      setBusy(false)
    }
  }

  return (
    <AccessSheet
      title="New dashboard"
      intro="It opens as a new tab on the project’s page. Add files to it here once it exists."
      onClose={onClose}
      busy={busy}
      testId="new-dashboard-dialog"
      footer={
        <>
            <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button variant="accent" size="md" disabled={busy || name.trim().length === 0} onClick={() => void submit()}>
              {busy ? 'Adding…' : 'Add dashboard'}
            </Button>
        </>
      }
    >
      <Field label="Name" htmlFor={fieldId}>
        <Input
          id={fieldId}
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
          }}
          placeholder="Release status"
          className={DIALOG_FIELD}
        />
      </Field>
      {error && (
        <FailedNote what="Couldn’t add the dashboard." next="Nothing was added. Change the name and try again." detail={error} />
      )}
    </AccessSheet>
  )
}

function ChevronIcon({ dir }: { dir: 'left' | 'right' }): React.JSX.Element {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
      <path d={dir === 'left' ? 'M10 3.5 5.5 8l4.5 4.5' : 'M6 3.5 10.5 8 6 12.5'} />
    </svg>
  )
}

function DashboardsSection({
  detail,
  caps,
  docs,
  docsError,
  onReloadDocs,
  bar = { stickBottom: true, stuck: null },
}: {
  detail: ProjectGroupShow
  caps: ProjectSettingsCaps
  docs: ProjectGroupHtmlDoc[] | null
  docsError: string | null
  onReloadDocs: () => void
  /** Where the bar sits in its scroller (useDashboardBar). */
  bar?: { stickBottom: boolean; stuck: 'top' | 'bottom' | null }
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
  const [creating, setCreating] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState<ProjectGroupDashboard | null>(null)
  /** The last failed dashboard write, shown under the row it came from. */
  const [failure, setFailure] = useState<{
    where: 'move' | 'delete' | string
    what: string
    reason: string
    retry?: () => void
    next?: string
  } | null>(null)
  /** What the last write did, for a moment ("Added to Main"). */
  const [done, setDone] = useState<{ where: string; text: string } | null>(null)
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (doneTimer.current) clearTimeout(doneTimer.current)
  }, [])
  const flash = (where: string, text: string): void => {
    if (doneTimer.current) clearTimeout(doneTimer.current)
    setDone({ where, text })
    doneTimer.current = setTimeout(() => setDone(null), 2500)
  }

  const reorder = useCallback(
    async (dashboardId: string, direction: -1 | 1): Promise<void> => {
      const order = moveDashboardId(orderedIds, dashboardId, direction)
      if (order === null || busy) return
      setBusy(true)
      setFailure(null)
      try {
        await reorderProjectGroupDashboards(primaryScope(), detail.id, order)
        flash('move', direction < 0 ? 'Moved earlier' : 'Moved later')
      } catch (err) {
        setFailure({ where: 'move', what: 'Couldn’t move the dashboard.', reason: errorMessage(err), retry: () => void reorder(dashboardId, direction) })
      } finally {
        setBusy(false)
      }
    },
    [detail.id, orderedIds, busy],
  )

  const deleteDashboard = useCallback(
    async (dashboard: ProjectGroupDashboard): Promise<void> => {
      setConfirmDelete(null)
      setFailure(null)
      try {
        await deleteProjectGroupDashboard(primaryScope(), detail.id, dashboard.id)
        setSelectedId(null)
        useToastStore.getState().addToast(`Deleted the dashboard “${dashboard.name}”.`, 'success')
      } catch (err) {
        const { code } = daemonErrorInfo(err)
        setFailure({
          where: 'delete',
          what: 'Couldn’t delete the dashboard.',
          reason: code === 'last_dashboard' ? 'a project keeps at least one dashboard' : errorMessage(err),
          retry: code === 'last_dashboard' ? undefined : () => setConfirmDelete(dashboard),
          next: code === 'last_dashboard' ? 'Add another dashboard first, then delete this one.' : undefined,
        })
      }
    },
    [detail.id],
  )

  const toggleDoc = useCallback(
    async (doc: ProjectGroupHtmlDoc, dashboard: ProjectGroupDashboard, shown: boolean): Promise<void> => {
      const key = `${doc.workspaceId}:${doc.filePath}`
      setDocBusy(key)
      setFailure(null)
      const pane = { kind: 'htmlDoc' as const, workspaceId: doc.workspaceId, filePath: doc.filePath }
      try {
        const result = shown
          ? await removeDashboardPane(primaryScope(), detail.id, dashboard.id, pane)
          : await addDashboardPane(primaryScope(), detail.id, dashboard.id, pane)
        setSeen((s) => ({ ...s, [result.id]: result }))
        flash(key, shown ? `Removed from ${dashboard.name}` : `Added to ${dashboard.name}`)
      } catch (err) {
        setFailure({
          where: key,
          what: `Couldn’t ${shown ? 'remove' : 'add'} ${doc.fileName}.`,
          reason: errorMessage(err),
          retry: () => void toggleDoc(doc, dashboard, shown),
        })
      } finally {
        setDocBusy(null)
      }
    },
    [detail.id],
  )

  const doneNote = (where: string): React.JSX.Element | null =>
    done?.where === where ? (
      <span role="status" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)] whitespace-nowrap">
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
          <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
        </svg>
        {done.text}
      </span>
    ) : null

  const failureFor = (where: string): React.ReactNode =>
    failure?.where === where ? <ActionError what={failure.what} reason={failure.reason} onRetry={failure.retry} next={failure.next} /> : undefined

  const filesState: React.ReactNode =
    docsError && docs === null ? (
      <SettingsLoadState
        kind="error"
        title="Couldn’t load this project’s files"
        detail={docsError}
        action={<Button variant="quiet" onClick={onReloadDocs}>Try again</Button>}
      >
        The files its agents shared can’t be listed right now.
      </SettingsLoadState>
    ) : docs === null ? (
      <SettingsLoadState kind="loading" title="Loading files…" />
    ) : docs.length === 0 ? (
      <SettingsLoadState kind="empty" title="No files yet">
        Files an agent in this project adds to its Files list show up here, ready to put on a dashboard.
      </SettingsLoadState>
    ) : undefined

  return (
    <>
      {creating && (
        <NewDashboardDialog
          groupId={detail.id}
          onClose={() => setCreating(false)}
          onCreated={(d) => {
            setCreating(false)
            setSelectedId(d.id)
            useToastStore.getState().addToast(`Added the dashboard “${d.name}”.`, 'success')
          }}
        />
      )}
      {confirmDelete && (
        <ConfirmSheet
          testId="delete-dashboard-confirm"
          title={`Delete the dashboard “${confirmDelete.name}”?`}
          message="Its tab and how it’s laid out go away. The agents and their files stay as they are."
          confirmLabel="Delete dashboard"
          destructive
          onCancel={() => setConfirmDelete(null)}
          onConfirm={() => void deleteDashboard(confirmDelete)}
        />
      )}

      {/* The selector bar: a DIRECT child of the left half, sticky at its
          top AND bottom, so it stays in view however far the half is
          scrolled (above it General, below it the dashboard). In a half too
          short for General and the bar together it sticks to the top only,
          so it never sits over General's rows. Held at an edge, it draws a
          hairline on that side: the content passes under it. */}
      <div
        data-testid="dashboard-selector"
        role="group"
        aria-label="Dashboard picker"
        data-settings-id="project-groups.dashboards"
        data-stuck={bar.stuck ?? undefined}
        className={cx(
          'sticky top-0 z-10 bg-[var(--color-bg)] pt-3 pb-2 space-y-2',
          bar.stickBottom && 'bottom-0',
          bar.stuck === 'top' && 'shadow-[0_1px_0_var(--color-border)]',
          bar.stuck === 'bottom' && 'shadow-[0_-1px_0_var(--color-border)]',
        )}
      >
        <div className="flex items-center justify-between gap-4 min-h-[1.5rem]">
          <h3 className="type-heading text-[var(--color-text-primary)]">Dashboards</h3>
          {!readOnly && (
            <Button variant="quiet" onClick={() => setCreating(true)}>
              New dashboard
            </Button>
          )}
        </div>
        <div className="flex items-center gap-2">
          <div className="flex-1 min-w-0">
            <SettingDropdown
              fullWidth
              menuAlign="left"
              ariaLabel="Dashboard"
              value={active?.id ?? ''}
              disabled={dashboards.length === 0}
              onChange={(value) => {
                setSelectedId(value)
                setFailure(null)
              }}
              options={
                dashboards.length === 0
                  ? [{ value: '', label: 'No dashboards' }]
                  : dashboards.map((d) => ({ value: d.id, label: d.name }))
              }
            />
          </div>
          {!readOnly && dashboards.length > 1 && active && (
            <>
              <Button
                variant="quiet"
                aria-label="Move dashboard earlier"
                title="Move this tab earlier"
                disabled={busy || activeIndex <= 0}
                onClick={() => void reorder(active.id, -1)}
                className="h-full"
              >
                <ChevronIcon dir="left" />
              </Button>
              <Button
                variant="quiet"
                aria-label="Move dashboard later"
                title="Move this tab later"
                disabled={busy || activeIndex >= dashboards.length - 1}
                onClick={() => void reorder(active.id, 1)}
                className="h-full"
              >
                <ChevronIcon dir="right" />
              </Button>
            </>
          )}
          {!readOnly && active && (
            <OverflowMenu
              label={`More actions for the dashboard ${active.name}`}
              items={[
                {
                  id: 'delete',
                  label: 'Delete dashboard…',
                  danger: true,
                  disabled: lastOne,
                  hint: lastOne ? 'A project keeps at least one dashboard.' : 'Removes this tab only. Files stay.',
                  onSelect: () => setConfirmDelete(active),
                },
              ]}
            />
          )}
        </div>
        {(done?.where === 'move' || failure?.where === 'move' || failure?.where === 'delete') && (
          <div>
            {doneNote('move')}
            {failureFor('move')}
            {failureFor('delete')}
          </div>
        )}
      </div>

      <section aria-label="Dashboards" className="space-y-6 pt-2">
        {active && (
          <>
            <SettingsGroup card>
              <NameField
                key={active.id}
                label="Dashboard"
                hint="The name on its tab."
                value={active.name}
                readOnly={readOnly}
                settingId="project-groups.dashboard-name"
                onSave={async (name) => {
                  await renameProjectGroupDashboard(primaryScope(), detail.id, active.id, name)
                }}
              />
            </SettingsGroup>

            <SettingsGroup
              card
              title="Files"
              hint={`HTML files your agents shared. Put one on ${active.name} to see it there.`}
              settingId="project-groups.dashboard-files"
              state={filesState}
            >
              {docs?.map((doc) => {
                const key = `${doc.workspaceId}:${doc.filePath}`
                const html = /\.html?$/i.test(doc.filePath)
                const shown = html && dashboardShowsDoc(layoutOf(active), detail.pocWorkspaceId, doc)
                const owner = doc.agentName ?? doc.workspaceName ?? ''
                return (
                  <SettingRow
                    key={key}
                    label={
                      <span className="flex items-center gap-2 min-w-0">
                        <span className="type-name truncate">{doc.fileName}</span>
                        {doc.missing ? <Tag tone="warn" data-case="as-written">Missing</Tag> : null}
                      </span>
                    }
                    hint={
                      html ? (
                        <span className="block truncate" title={doc.filePath}>
                          {owner ? `From ${owner}` : doc.filePath}
                        </span>
                      ) : (
                        'Not an HTML file, so it can’t go on a dashboard.'
                      )
                    }
                    below={failureFor(key)}
                  >
                    {doneNote(key)}
                    {html && !readOnly && (
                      <Button
                        variant="quiet"
                        disabled={docBusy === key}
                        onClick={() => void toggleDoc(doc, active, shown)}
                      >
                        {docBusy === key ? 'Saving…' : shown ? `Remove from ${active.name}` : `Add to ${active.name}`}
                      </Button>
                    )}
                    {html && readOnly && shown && <Tag tone="neutral" data-case="as-written">On {active.name}</Tag>}
                  </SettingRow>
                )
              })}
            </SettingsGroup>
          </>
        )}
      </section>
    </>
  )
}

// ── Agents (right half) ──────────────────────────────────────────────────

/** A folder, short and steady: the home folder as `~`, so every row starts
 *  the same way (`~/Projects/web-app`). The full path is the title. */
export function shortFolder(path: string): string {
  return path.replace(/^\/(?:Users|home)\/[^/]+(?=\/|$)/, '~')
}

/** An agent's folder on its row's second line: short, cut at the end if it
 *  must be, the full path on hover. */
function FolderLine({ path }: { path: string }): React.JSX.Element {
  return (
    <span className="block truncate" title={path}>
      {shortFolder(path)}
    </span>
  )
}

/** An agent's name in the Add an agent list: its display name (AGENT.md),
 *  as the project's own agents show theirs, not the sidebar's folder name.
 *  projects/list carries it on every row (`agentDisplayName`); an older
 *  daemon doesn't, and the row shows the sidebar name. */
export function addableAgentLabel(w: { name: string; agentDisplayName?: string }): string {
  return w.agentDisplayName?.trim() || w.name
}

const PLUS = (
  <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-3 h-3 flex-shrink-0">
    <path d="M8 3.5v9M3.5 8h9" />
  </svg>
)

/** "Add an agent": search the agents on this server that aren't in the
 *  project yet and add one with a click (or the arrows and Enter). Settings
 *  → Projects shows it in its Agents half; the Projects page shows the same
 *  picker in a dialog from the project bar's Agents menu. */
export function AddAgentPicker({
  groupId,
  groupName,
  memberIds,
  onAdded,
  listClassName = 'max-h-80',
  inset = 'px-4',
  searchRef,
}: {
  groupId: string
  groupName: string
  memberIds: string[]
  /** After the daemon took it (the page refetches the project). */
  onAdded?: (workspaceId: string, displayName: string) => void
  /** The list's height cap. */
  listClassName?: string
  /** The rows' side padding (a card's 16px, or a dialog's own edge). */
  inset?: string
  /** The search field (Settings' Agents heading jumps to it). */
  searchRef?: React.Ref<HTMLInputElement>
}): React.JSX.Element {
  const registered = useProjectsStore((s) => s.projects)
  const [query, setQuery] = useState('')
  // -1 = no row picked yet: a row lights up once you type or use the arrows.
  const [highlight, setHighlight] = useState(-1)
  const [adding, setAdding] = useState<string | null>(null)
  const [failure, setFailure] = useState<{ what: string; reason: string; retry: () => void } | null>(null)
  /** The agent just added, said under the search for a moment. */
  const [added, setAdded] = useState<string | null>(null)
  const addedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (addedTimer.current) clearTimeout(addedTimer.current)
  }, [])
  const outside = useMemo(() => addableWorkspaces(registered, memberIds), [registered, memberIds])
  const labelOf = addableAgentLabel
  // The search matches what the row shows (the display name) as well as
  // the sidebar name and the folder.
  const q = query.trim().toLowerCase()
  const candidates = q
    ? outside.filter((w) => labelOf(w).toLowerCase().includes(q) || addableWorkspaces([w], [], q).length > 0)
    : outside
  useEffect(() => setHighlight(query.trim() ? 0 : -1), [query])

  const addMember = useCallback(
    async (workspaceId: string, displayName: string): Promise<void> => {
      setAdding(workspaceId)
      setFailure(null)
      try {
        await addProjectGroupMember(primaryScope(), groupId, workspaceId)
        setQuery('')
        if (addedTimer.current) clearTimeout(addedTimer.current)
        setAdded(displayName)
        addedTimer.current = setTimeout(() => setAdded(null), 4000)
        useToastStore.getState().addToast(`Added ${displayName} to ${groupName}.`, 'success')
        onAdded?.(workspaceId, displayName)
      } catch (err) {
        setFailure({
          what: `Couldn’t add ${displayName}.`,
          reason: errorMessage(err),
          retry: () => void addMember(workspaceId, displayName),
        })
      } finally {
        setAdding(null)
      }
    },
    [groupId, groupName, onAdded],
  )

  return (
    <div data-testid="add-agent-picker">
      <div className={cx('py-3', inset)}>
        <SearchField
          ref={searchRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onClear={() => setQuery('')}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') {
              e.preventDefault()
              setHighlight((h) => Math.min(h + 1, candidates.length - 1))
            } else if (e.key === 'ArrowUp') {
              e.preventDefault()
              setHighlight((h) => Math.max(h - 1, 0))
            } else if (e.key === 'Enter' && candidates[highlight]) {
              e.preventDefault()
              void addMember(candidates[highlight].id, labelOf(candidates[highlight]))
            }
          }}
          aria-label="Search agents to add"
          placeholder="Search agents"
          className="w-full"
        />
        {failure ? (
          <div className="mt-2">
            <ActionError what={failure.what} reason={failure.reason} onRetry={failure.retry} />
          </div>
        ) : added ? (
          <div className="mt-2">
            <DoneNote text={`Added ${added}`} />
          </div>
        ) : null}
      </div>
      <ul
        className={cx(listClassName, 'overflow-y-auto divide-y divide-[var(--color-border)]')}
        aria-label="Agents you can add"
        // The pointer leaving drops a hover pick (a typed search keeps its first match).
        onMouseLeave={() => setHighlight(query.trim() ? 0 : -1)}
      >
        {/* Each row is one quiet button: the agent first, "Add" in the
            hint colour until the row is hovered, focused or picked by
            the arrows, so the project's own agents above read first. */}
        {candidates.map((w, i) => (
          <li key={w.id} data-highlighted={i === highlight || undefined} onMouseEnter={() => setHighlight(i)}>
            <button
              type="button"
              data-k2-focus=""
              aria-label={`Add ${labelOf(w)}`}
              title={w.path}
              disabled={adding === w.id}
              onClick={() => void addMember(w.id, labelOf(w))}
              className={cx(
                'group relative w-full flex items-center gap-4 py-2.5 text-left no-drag cursor-pointer outline-none transition-colors disabled:cursor-wait',
                inset,
                i === highlight ? SETTING_LIST_ROW_BAND : SETTING_LIST_ROW_HOVER,
              )}
            >
              <span className="flex-1 min-w-0">
                <span className="block type-name text-[var(--color-text-primary)] truncate">{labelOf(w)}</span>
                <span className="block type-hint text-[var(--color-text-secondary)] truncate">{shortFolder(w.path)}</span>
              </span>
              <span
                className={cx(
                  'flex-shrink-0 inline-flex items-center gap-1 type-control transition-colors',
                  i === highlight
                    ? 'text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-secondary)] group-hover:text-[var(--color-text-primary)] group-focus-visible:text-[var(--color-text-primary)]',
                )}
              >
                {adding === w.id ? (
                  'Adding…'
                ) : (
                  <>
                    {PLUS}
                    Add
                  </>
                )}
              </span>
            </button>
          </li>
        ))}
        {candidates.length === 0 && (
          <li className={cx('py-3', inset)}>
            <div className="type-row text-[var(--color-text-primary)]">
              {outside.length === 0 ? 'Every agent is already here' : `No agents match “${query.trim()}”`}
            </div>
            <div className={SETTING_ROW_HINT}>
              {outside.length === 0
                ? 'Every agent on this server is already in this project.'
                : 'Try another name or folder, or clear the search to see them all.'}
            </div>
          </li>
        )}
      </ul>
    </div>
  )
}

/** "Added", "Moved later": what a change just did, by the control, in the
 *  ok colour with a check, for a moment. */
function DoneNote({ text }: { text: string }): React.JSX.Element {
  return (
    <span role="status" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)] whitespace-nowrap">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      {text}
    </span>
  )
}

/** An agent row's "Opens in": the view its tile opens in for everyone,
 *  marked "(project default)" when it follows the project, and a menu to
 *  set it or follow the project again (member-view.ts). Read-only text,
 *  with the reason on hover, for someone who can't change it. */
function OpensInMenu({
  name,
  value,
  projectView,
  lockReason,
  busy,
  onPick,
}: {
  name: string
  value: MemberView
  projectView: ProjectDefaultView
  lockReason: string | null
  busy: boolean
  onPick: (next: ProjectDefaultView | null) => void
}): React.JSX.Element {
  const inherited = value.own === null
  const text = (
    <>
      <span className="flex-shrink-0 inline-flex [&>svg]:w-3 [&>svg]:h-3" aria-hidden="true">
        <ViewIcon kind={value.view} />
      </span>
      <span className="whitespace-nowrap">
        {MEMBER_VIEW_NAME[value.view]}
        {inherited && <span className="text-[var(--color-text-secondary)]"> (default)</span>}
      </span>
    </>
  )
  if (lockReason) {
    return (
      <span
        className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)]"
        title={`Opens in ${MEMBER_VIEW_NAME[value.view]}${inherited ? ', the project default' : ''}. ${lockReason}`}
        data-testid="agent-opens-in"
        data-locked=""
      >
        <span className="sr-only">Opens in</span>
        {text}
        <span className="sr-only">. {lockReason}</span>
      </span>
    )
  }
  return (
    <OverflowMenu
      label={`Opens in ${MEMBER_VIEW_NAME[value.view]}${inherited ? ' (project default)' : ''}: change where ${name} opens`}
      menuLabel="Opens in"
      items={[
        {
          id: 'default',
          label: projectDefaultLabel(projectView),
          hint: 'Follows the project’s view.',
          checked: inherited,
          disabled: busy,
          onSelect: () => onPick(null),
        },
        ...PROJECT_DEFAULT_VIEWS.map((v, i) => ({
          id: v,
          label: MEMBER_VIEW_NAME[v],
          group: i === 0,
          icon: <ViewIcon kind={v} />,
          checked: value.own === v,
          disabled: busy,
          onSelect: () => onPick(v),
        })),
      ]}
      renderTrigger={(t) => (
        <Button
          ref={t.ref}
          variant="quiet"
          size="sm"
          aria-haspopup={t['aria-haspopup']}
          aria-expanded={t['aria-expanded']}
          onClick={t.onClick}
          disabled={busy}
          title="Where this agent’s tile opens, for everyone. Anyone can still switch their own."
          className="inline-flex items-center gap-1.5"
          data-testid="agent-opens-in"
          data-own-view={value.own ?? undefined}
        >
          <span className="text-[var(--color-text-secondary)]">Opens in</span>
          {text}
        </Button>
      )}
    />
  )
}

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
  const [memberBusy, setMemberBusy] = useState<string | null>(null)
  const [confirmRemove, setConfirmRemove] = useState<{ id: string; name: string } | null>(null)
  const [failure, setFailure] = useState<{
    id: string
    what: string
    reason: string
    retry?: () => void
    /** A refusal said in one sentence (trying again won't help). */
    plain?: boolean
  } | null>(null)
  /** What the last change did, on the row it changed, for a moment. */
  const [done, setDone] = useState<{ id: string; text: string } | null>(null)
  const doneTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (doneTimer.current) clearTimeout(doneTimer.current)
  }, [])
  const flash = (id: string, text: string): void => {
    if (doneTimer.current) clearTimeout(doneTimer.current)
    setDone({ id, text })
    doneTimer.current = setTimeout(() => setDone(null), 4000)
  }
  // 0302: where each agent opens, for everyone (owner or admin).
  const views = useMemberViews(detail, primaryScope())
  const projectView = projectDefaultView(detail)
  const viewLockReason = memberViewLockReason(caps.canEditLooks, daemonKnowsMemberViews(detail.members))
  const setView = async (m: ProjectGroupMemberInfo, display: string, next: ProjectDefaultView | null): Promise<void> => {
    setFailure(null)
    try {
      await views.set(m, next)
      flash(m.workspaceId, next === null ? `Opens in ${MEMBER_VIEW_NAME[projectView]} now` : `Opens in ${MEMBER_VIEW_NAME[next]} now`)
      useToastStore.getState().addToast(memberViewSavedText(display, next, projectView), 'success')
    } catch (err) {
      const e = memberViewError(err, display)
      setFailure(
        'plain' in e
          ? { id: m.workspaceId, what: e.plain, reason: '', plain: true }
          : {
              id: m.workspaceId,
              what: `Couldn’t change where ${display} opens.`,
              reason: e.reason,
              retry: () => void setView(m, display, next),
            },
      )
    }
  }
  const addSearchRef = useRef<HTMLInputElement>(null)
  /** The Agents heading's "Add an agent": to the picker below, ready to type. */
  const goToPicker = (): void => {
    const field = addSearchRef.current
    if (!field) return
    field.scrollIntoView?.({ block: 'center', behavior: 'smooth' })
    field.focus({ preventScroll: true })
  }

  const removeMember = useCallback(
    async (workspaceId: string, displayName: string): Promise<void> => {
      setConfirmRemove(null)
      setMemberBusy(workspaceId)
      setFailure(null)
      try {
        await removeProjectGroupMember(primaryScope(), detail.id, workspaceId)
        useToastStore.getState().addToast(`Removed ${displayName} from ${detail.name}.`, 'success')
      } catch (err) {
        setFailure({
          id: workspaceId,
          what: `Couldn’t remove ${displayName}.`,
          reason: errorMessage(err),
          retry: () => setConfirmRemove({ id: workspaceId, name: displayName }),
        })
      } finally {
        setMemberBusy(null)
      }
    },
    [detail.id, detail.name],
  )

  const setPoc = useCallback(
    async (workspaceId: string, displayName: string): Promise<void> => {
      setMemberBusy(workspaceId)
      setFailure(null)
      try {
        await edit({ pocWorkspaceId: workspaceId }, () =>
          setProjectGroupPoc(primaryScope(), detail.id, workspaceId),
        )
        flash(workspaceId, 'Gets the project chat now')
        useToastStore
          .getState()
          .addToast(`${displayName} is now the point of contact and gets the project chat.`, 'success')
      } catch (err) {
        useToastStore.getState().addToast('Couldn’t change the point of contact. Nothing changed.', 'error')
        setFailure({
          id: workspaceId,
          what: 'Couldn’t change the point of contact.',
          reason: errorMessage(err),
          retry: () => void setPoc(workspaceId, displayName),
        })
      } finally {
        setMemberBusy(null)
      }
    },
    [detail.id, edit],
  )

  return (
    <section aria-label="Agents" className="space-y-6" data-settings-id="project-groups.agents">
      {confirmRemove && (
        <ConfirmSheet
          testId="remove-agent-confirm"
          title={`Remove ${confirmRemove.name} from ${detail.name}?`}
          message="It stops getting this project’s chat and leaves its dashboards. The agent and its folder stay as they are, and you can add it back any time."
          confirmLabel="Remove from project"
          destructive
          onCancel={() => setConfirmRemove(null)}
          onConfirm={() => void removeMember(confirmRemove.id, confirmRemove.name)}
        />
      )}
      <SettingsGroup
        card
        title="Agents"
        hint="The point of contact gets every message posted in the project’s chat."
        action={
          readOnly ? undefined : (
            <Button variant="quiet" onClick={goToPicker} className="inline-flex items-center gap-1.5">
              {PLUS}
              Add an agent
            </Button>
          )
        }
        state={
          detail.members.length === 0 ? (
            <SettingsLoadState kind="empty" title="No agents yet">
              The first agent you add becomes the point of contact.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {detail.members.map((m) => {
          const isPoc = m.workspaceId === detail.pocWorkspaceId
          const blocked = removeMemberBlockedReason(m.workspaceId, detail.pocWorkspaceId)
          const display = m.agentName ?? m.name ?? m.workspaceId.slice(0, 8)
          return (
            <div key={m.workspaceId} data-testid="project-agent-row" className="group/agent">
              <SettingRow
                label={
                  <span className="flex items-center gap-2 min-w-0">
                    <span className="type-name truncate">{display}</span>
                    {isPoc && <Tag tone="accent" data-case="as-written" className="flex-shrink-0">Point of contact</Tag>}
                  </span>
                }
                hint={m.path ? <FolderLine path={m.path} /> : 'Not on this server any more'}
                below={
                  failure?.id === m.workspaceId ? (
                    failure.plain ? (
                      <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">
                        {failure.what}
                      </p>
                    ) : (
                      <ActionError what={failure.what} reason={failure.reason} onRetry={failure.retry} />
                    )
                  ) : undefined
                }
              >
                {memberBusy === m.workspaceId || views.busy === m.workspaceId ? <SavingNote on /> : null}
                {done?.id === m.workspaceId && memberBusy !== m.workspaceId && views.busy !== m.workspaceId ? (
                  <DoneNote text={done.text} />
                ) : null}
                <OpensInMenu
                  name={display}
                  value={views.viewOf(m)}
                  projectView={projectView}
                  lockReason={viewLockReason}
                  busy={views.busy === m.workspaceId}
                  onPick={(next) => {
                    const cur = views.viewOf(m)
                    if (next === cur.own) return
                    void setView(m, display, next)
                  }}
                />
                {/* Shown on the row's hover and keyboard focus only, so the
                    rows read as names first; the … menu keeps it too. */}
                {!readOnly && !isPoc && (
                  <Button
                    variant="ghost"
                    size="sm"
                    data-reveal="row"
                    className="opacity-0 transition-opacity group-hover/agent:opacity-100 group-focus-within/agent:opacity-100 focus-visible:opacity-100"
                    disabled={memberBusy === m.workspaceId}
                    title="It gets every message in the project’s chat."
                    onClick={() => void setPoc(m.workspaceId, display)}
                  >
                    Make point of contact
                  </Button>
                )}
                {!readOnly && (
                  <OverflowMenu
                    label={`More actions for ${display}`}
                    items={[
                      ...(isPoc
                        ? []
                        : [
                            {
                              id: 'poc',
                              label: 'Make point of contact',
                              hint: 'It gets every message in the project’s chat.',
                              disabled: memberBusy === m.workspaceId,
                              onSelect: () => void setPoc(m.workspaceId, display),
                            },
                          ]),
                      {
                        id: 'remove',
                        label: 'Remove from project…',
                        danger: true,
                        group: !isPoc,
                        disabled: blocked !== null || memberBusy === m.workspaceId,
                        hint: blocked ?? 'The agent and its folder stay.',
                        onSelect: () => setConfirmRemove({ id: m.workspaceId, name: display }),
                      },
                    ]}
                  />
                )}
              </SettingRow>
            </div>
          )
        })}
      </SettingsGroup>

      {!readOnly && (
        <SettingsGroup
          card
          title="Add an agent"
          hint="Pick an agent on this server to join the project and its chat."
          settingId="project-groups.add-agent"
        >
          <AddAgentPicker
            groupId={detail.id}
            groupName={detail.name}
            memberIds={detail.members.map((m) => m.workspaceId)}
            searchRef={addSearchRef}
            onAdded={(workspaceId) => flash(workspaceId, 'Added')}
          />
        </SettingsGroup>
      )}
    </section>
  )
}

// ── Agents open in (prd-projects-threads-v1 §4.4) ────────────────────────

const DEFAULT_VIEW_OPTIONS: { value: ProjectDefaultView; label: string }[] = [
  { value: 'terminal', label: 'Terminal' },
  { value: 'thread', label: 'Thread' },
  { value: 'chatter', label: 'Chatter' },
]

/** What the three views are, once, and that this is only where they start. */
const DEFAULT_VIEW_HINT =
  'Terminal is the agent’s shell, Thread its chat with you, Chatter its messages with other agents. Anyone can still switch their own.'

/** Why a save was refused, for this row: a refusal by role or permission
 *  is one plain sentence (trying again won't help); anything else is the
 *  shared failure (plain words, the server's reason quietly, Try again). */
type DefaultViewError = { plain: string } | { reason: string; tried: ProjectDefaultView }

function defaultViewError(err: unknown, tried: ProjectDefaultView): DefaultViewError {
  // A refusal arrives as a DaemonHttpError carrying the code; the older
  // JSON-in-the-message shape is read too.
  const code = (err instanceof DaemonHttpError ? err.code : null) ?? daemonErrorInfo(err).code
  if (code === 'forbidden') return { plain: 'Couldn’t change it. Only the owner or an admin can.' }
  if (code === 'agent_permission_off') return { plain: `Couldn’t change it. This agent needs Manage ${PRODUCT_NAME} projects on.` }
  return { reason: errorMessage(err), tried }
}

/** The view every agent tile in this project opens in. Daemon-owned (the
 *  `defaultView` column), so every client and the phone agree. Optimistic
 *  through `edit`: the choice shows at once and rolls back with an inline
 *  error when the daemon refuses. A saved change drops this person's tile
 *  switches against the old default; other clients drop theirs when the
 *  groups-changed refetch shows the new default. */
function DefaultViewRow({
  detail,
  caps,
  edit,
  narrow = false,
}: {
  detail: ProjectGroupShow
  caps: ProjectSettingsCaps
  edit: GroupEdit
  /** In the narrow General column the choices go under the label (a
   *  stacked row), so the hint keeps its width instead of wrapping beside
   *  them. */
  narrow?: boolean
}): React.JSX.Element {
  const value = projectDefaultView(detail)
  const lockReason = defaultViewLockReason(caps, detail.defaultView !== undefined)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<DefaultViewError | null>(null)
  const save = useSaveStatus()
  const groupId = detail.id
  const shownId = useRef(groupId)
  shownId.current = groupId

  // Another project selected: its row starts clean, and a late answer for
  // the previous one never lands here.
  useEffect(() => {
    setError(null)
    setSaving(false)
  }, [groupId])

  const change = (next: ProjectDefaultView): void => {
    if (next === value || lockReason) return
    setError(null)
    save.reset()
    setSaving(true)
    edit({ defaultView: next }, () => setProjectGroupDefaultView(primaryScope(), groupId, next))
      .then(() => {
        useProjectGroupsStore.getState().dropStaleTileViews(groupId, next, memberOwnViews(detail.members))
        if (shownId.current === groupId) save.saved()
      })
      .catch((err) => {
        if (shownId.current === groupId) setError(defaultViewError(err, next))
      })
      .finally(() => {
        if (shownId.current === groupId) setSaving(false)
      })
  }

  const below =
    error && 'plain' in error ? (
      <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed">
        {error.plain}
      </p>
    ) : error ? (
      <ActionError what="Couldn’t change how agents open." reason={error.reason} onRetry={() => change(error.tried)} />
    ) : lockReason ? (
      <p className={cx(SETTING_ROW_HINT, '!mt-0')} data-testid="project-default-view-locked">
        {lockReason}
      </p>
    ) : undefined
  const control = (
    <Segmented
      aria-label="Agents open in"
      value={value}
      options={DEFAULT_VIEW_OPTIONS}
      activation="manual"
      disabled={lockReason !== null}
      onChange={change}
    />
  )
  const notes = (
    <>
      <SavingNote on={saving} />
      <SaveNote state={saving ? 'idle' : save.state} />
    </>
  )

  // A card row: label and hint, the choices on the shared right edge, or
  // under the hint in the narrow column (stacked), with "Saving…" / "Saved"
  // beside them; a lock reason or an error takes its own line under the row.
  return (
    <SettingRow
      label="Agents open in"
      hint={DEFAULT_VIEW_HINT}
      settingId="project-groups.default-view"
      testId="project-default-view-row"
      below={below}
      stack={narrow}
    >
      {narrow ? control : notes}
      {narrow ? notes : control}
    </SettingRow>
  )
}

// ── The right panel (selected project's management) ──────────────────────

/** Optimistic group edit: apply `patch` now, run the daemon call, take its
 *  answer, or roll back and rethrow. */
type GroupEdit = (patch: Partial<ProjectGroup>, run: () => Promise<ProjectGroup | void>) => Promise<void>

/** One colour chip of the Color row: a 24px chip (cut in Terminal, round in
 *  Glass) inside a 28px button, so the keyboard ring sits 2px clear of the
 *  colour in every look instead of on it. */
function Swatch({
  color,
  label,
  selected,
  title,
  onClick,
  children,
}: {
  color: string
  label: string
  selected: boolean
  title?: string
  onClick: () => void
  children?: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={selected}
      title={title ?? label}
      onClick={onClick}
      data-k2-chamfer="chip"
      className="k2-focus no-drag cursor-pointer w-7 h-7 p-0.5 flex-shrink-0 rounded-[var(--radius-pill)] bg-transparent border-transparent"
    >
      <span
        data-k2-chamfer="chip"
        className="w-6 h-6 flex items-center justify-center rounded-[var(--radius-pill)] border-transparent type-badge text-[var(--color-on-accent)]"
        style={{ backgroundColor: color }}
      >
        {selected ? (
          <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="var(--color-on-accent)" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M3 8.5l3.2 3.2L13 5" />
          </svg>
        ) : (
          children
        )}
      </span>
    </button>
  )
}

export function ProjectSettingsDetail({
  detail,
  role,
  onDeleted,
  edit,
  docs,
  docsError,
  onReloadDocs,
  notice,
  variant = 'page',
  sheetActions,
  titleId,
}: {
  detail: ProjectGroupShow
  role: ProjectRole | null
  onDeleted: (name: string) => void
  edit: GroupEdit
  /** Workspace Resources (the dashboard "Files" list) — fetched by the
   *  page alongside show (0.46.1); null = not loaded yet. */
  docs: ProjectGroupHtmlDoc[] | null
  docsError: string | null
  onReloadDocs: () => void
  /** What the page just did (a project created or deleted), once. */
  notice: string | null
  /** `sheet`: shown over the Projects page (ProjectSettingsSheet). Its
   *  title row ends in the sheet's own actions ("Open in Settings", the
   *  close ×) instead of "Open project" (the project is open behind it). */
  variant?: 'page' | 'sheet'
  /** The sheet's title-row actions (with `variant="sheet"`). */
  sheetActions?: React.ReactNode
  /** The sheet labels itself with the title row's heading. */
  titleId?: string
}): React.JSX.Element {
  const inSheet = variant === 'sheet'
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
  /** A picture write in flight: an upload or a removal. */
  const [iconBusy, setIconBusy] = useState<'upload' | 'remove' | null>(null)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const iconInputRef = useRef<HTMLInputElement>(null)
  const pictureSave = useSaveStatus()
  /** The last picture write that failed, and what it sent, for Try again. */
  const [pictureFailure, setPictureFailure] = useState<{ reason: string; dataUrl: string | null } | null>(null)
  const colorSave = useSaveStatus()
  /** The last colour write that failed, and the colour, for Try again. */
  const [colorFailure, setColorFailure] = useState<{ reason: string; color: string | null } | null>(null)
  const pinSave = useSaveStatus()

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
      setIconBusy(dataUrl === null ? 'remove' : 'upload')
      setPictureFailure(null)
      pictureSave.reset()
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
        pictureSave.saved()
      } catch (err) {
        setPictureFailure({ reason: errorMessage(err), dataUrl })
      } finally {
        setIconBusy(null)
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [detail.id, hostKey],
  )

  // Custom color field (§6.7.7 — the palette + a free `#rrggbb` / rgb()
  // field; Enter or leaving it applies, a value that isn't a color says so).
  const [customHex, setCustomHex] = useState('')
  const [hexError, setHexError] = useState(false)

  const saveColor = useCallback(
    async (color: string | null): Promise<void> => {
      if (color === detail.color) return
      setColorFailure(null)
      colorSave.reset()
      try {
        // Optimistic: the swatch, the avatars and the list move at once;
        // a refusal puts the old color back and says why.
        await edit({ color }, () => setProjectGroupColor(primaryScope(), detail.id, color))
        colorSave.saved()
      } catch (err) {
        setColorFailure({ reason: errorMessage(err), color })
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [detail.id, detail.color, edit],
  )

  const commitCustomHex = useCallback((): void => {
    if (!customHex.trim()) {
      setHexError(false)
      return
    }
    const normalized = normalizeHexColor(customHex)
    if (normalized === null) {
      setHexError(true)
      return
    }
    setHexError(false)
    setCustomHex('')
    void saveColor(normalized)
  }, [customHex, saveColor])

  const [confirmDelete, setConfirmDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [deleteError, setDeleteError] = useState<string | null>(null)
  const deleteProject = useCallback(async (): Promise<void> => {
    setConfirmDelete(false)
    setDeleting(true)
    setDeleteError(null)
    try {
      await deleteProjectGroup(primaryScope(), detail.id)
      onDeleted(detail.name)
    } catch (err) {
      setDeleteError(errorMessage(err))
      setDeleting(false)
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

  const [bodyRef, bodyWidth] = useCssWidth(HALVES_MIN_WIDTH)
  const stacked = bodyWidth < HALVES_MIN_WIDTH
  const leftRef = useRef<HTMLDivElement>(null)
  const generalRef = useRef<HTMLDivElement>(null)
  const bar = useDashboardBar(stacked ? bodyRef : leftRef, generalRef, stacked ? 'stacked' : 'split')

  const general = (
    <section aria-label="General">
      <SettingsGroup card title="General" hint="Rename, picture, how agents open, and pinning.">
        <NameField
          label="Project"
          hint="In the sidebar."
          value={detail.name}
          readOnly={!caps.canEdit}
          settingId="project-groups.name"
          onSave={(name) => edit({ name }, () => renameProjectGroup(primaryScope(), detail.id, name))}
        />
        <SettingRow
          label="Picture"
          hint={caps.canEditLooks ? 'Shown in the sidebar. Square images work best.' : 'Shown in the sidebar.'}
          settingId="project-groups.picture"
          below={
            pictureFailure ? (
              <ActionError
                what={pictureFailure.dataUrl ? 'Couldn’t save the picture.' : 'Couldn’t remove the picture.'}
                reason={pictureFailure.reason}
                onRetry={() => void saveIcon(pictureFailure.dataUrl)}
              />
            ) : undefined
          }
        >
          {iconBusy && <BusyNote text={iconBusy === 'remove' ? 'Removing…' : 'Uploading…'} />}
          <SaveNote state={iconBusy ? 'idle' : pictureSave.state} />
          <span className={cx(AVATAR_ROUND, 'transition-opacity', iconBusy && 'opacity-60')}>
            <ProjectGroupAvatar
              name={detail.name}
              groupId={detail.id}
              size={32}
              color={detail.color}
              iconUrl={icon?.dataUrl ?? null}
            />
          </span>
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
                disabled={iconBusy !== null}
              >
                Upload
              </Button>
              <input
                ref={iconInputRef}
                type="file"
                accept="image/png,image/jpeg,image/svg+xml,image/x-icon"
                className="hidden"
                onChange={handleIconFileSelected}
              />
              {icon?.dataUrl && (
                <Button
                  variant="quiet"
                  onClick={() => void saveIcon(null)}
                  disabled={iconBusy !== null}
                  title="Remove the picture"
                >
                  Remove
                </Button>
              )}
            </>
          )}
        </SettingRow>
        <SettingRow
          label="Color"
          hint="Behind the project’s initial when it has no picture, and around the picture when it has one."
          settingId="project-groups.color"
          below={
            caps.canEditLooks ? (
              <div className="space-y-1.5">
                <div className="flex flex-wrap items-center gap-0.5 -ml-0.5">
                  <Swatch
                    color={groupAvatarColor(detail.id)}
                    label="Automatic color"
                    title={`Automatic: a steady color ${PRODUCT_NAME} picks for this project`}
                    selected={detail.color === null}
                    onClick={() => void saveColor(null)}
                  >
                    A
                  </Swatch>
                  {GROUP_AVATAR_COLORS.map((color) => (
                    <Swatch
                      key={color}
                      color={color}
                      label={`Color ${color}`}
                      selected={detail.color === color}
                      onClick={() => void saveColor(color)}
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
                    placeholder={detail.color ?? '#rrggbb'}
                    spellCheck={false}
                    aria-invalid={hexError || undefined}
                    className={cx('w-24 type-control ml-2', hexError && '!border-[var(--color-status-error-soft)]')}
                  />
                </div>
                {hexError && (
                  <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
                    That isn’t a color. Type six hex digits, or an rgb() value.
                  </p>
                )}
                {colorFailure && (
                  <ActionError
                    what="Couldn’t change the color."
                    reason={colorFailure.reason}
                    onRetry={() => void saveColor(colorFailure.color)}
                  />
                )}
              </div>
            ) : undefined
          }
        >
          {caps.canEditLooks ? (
            <SaveNote state={colorSave.state} />
          ) : (
            <span className="flex items-center gap-2 type-row text-[var(--color-text-primary)]">
              <span
                data-k2-chamfer="chip"
                className="w-5 h-5 flex-shrink-0 inline-block rounded-[var(--radius-pill)]"
                style={{ backgroundColor: effectiveColor }}
              />
              {detail.color ?? 'Automatic'}
            </span>
          )}
        </SettingRow>
        <DefaultViewRow detail={detail} caps={caps} edit={edit} narrow={!stacked} />
        <SettingRow
          label="Show in Pinned"
          hint="Keeps it at the top of the list for everyone."
          settingId="project-groups.pinned"
          below={belowFor(pinSave.state)}
        >
          <SaveNote state={pinSave.state} />
          <Toggle
            aria-label="Show in Pinned"
            checked={detail.pinned}
            disabled={!caps.canEdit}
            onChange={(next) => {
              pinSave.reset()
              void edit({ pinned: next }, () => pinProjectGroup(primaryScope(), detail.id, next))
                .then(() => pinSave.saved())
                .catch((err) => {
                  pinSave.failed(new Error(errorMessage(err)))
                  useToastStore.getState().addToast("Couldn't change Pinned. Nothing changed.", 'error')
                })
            }}
          />
        </SettingRow>
        <SettingRow
          label="Delete project"
          hint={
            caps.canDelete
              ? 'Its dashboards and chat go too. Its agents stay.'
              : 'Only the owner or an admin can delete a project.'
          }
          settingId="project-groups.delete"
          below={
            deleteError ? (
              <ActionError what="Couldn’t delete the project." reason={deleteError} onRetry={() => setConfirmDelete(true)} />
            ) : undefined
          }
        >
          {deleting ? (
            <BusyNote text="Deleting…" />
          ) : (
            <Button
              variant="quiet"
              disabled={!caps.canDelete}
              title={caps.canDelete ? undefined : 'Only the owner or an admin can delete a project'}
              onClick={() => setConfirmDelete(true)}
            >
              Delete project
            </Button>
          )}
        </SettingRow>
      </SettingsGroup>
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
      {confirmDelete && (
        <ConfirmSheet
          testId="delete-project-confirm"
          title={`Delete the project “${detail.name}”?`}
          message="Its dashboards and its whole chat go away for everyone on this server. The agents and their folders stay as they are. This can’t be undone."
          confirmLabel="Delete project"
          destructive
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void deleteProject()}
        />
      )}
      <div className="flex flex-col h-full min-h-0" data-testid="project-settings-detail">
        {/* ── Header (fixed; never scrolls away) ── */}
        <div
          className={cx(
            'flex-shrink-0 px-6 space-y-4 border-b border-[var(--color-border)]',
            inSheet ? 'pt-4 pb-4 pr-4' : 'pt-6 pb-4',
          )}
          data-testid="project-settings-head"
        >
          <div className="flex items-center gap-3 min-w-0">
            <span className={AVATAR_ROUND}>
              <ProjectGroupAvatar
                name={detail.name}
                groupId={detail.id}
                size={inSheet ? 40 : 36}
                color={detail.color}
                iconUrl={icon?.dataUrl ?? null}
              />
            </span>
            <div className="flex-1 min-w-0">
              <h2 id={titleId} className="type-title text-[var(--color-text-primary)] truncate">
                {detail.name}
              </h2>
              <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 truncate">
                {detail.memberCount} {detail.memberCount === 1 ? 'agent' : 'agents'}
                {detail.pocWorkspaceId
                  ? ` · Point of contact: ${memberLabel(detail.pocWorkspaceId)}`
                  : ' · No point of contact yet'}
              </p>
            </div>
            {inSheet ? (
              sheetActions
            ) : (
              <Button variant="quiet" onClick={openProject}>
                Open project
              </Button>
            )}
          </div>
          {notice && (
            <Callout tone="ok" role="status" className="type-hint" data-testid="project-settings-notice">
              {notice}
            </Callout>
          )}
          {banner && (
            <Callout tone="info" className="type-hint" data-testid="project-settings-readonly-banner">
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
            halves.body(stacked),
            // In the sheet the body is the page's own ground, as in Settings,
            // so its cards and the sticky dashboard bar read the same.
            inSheet && 'bg-[var(--color-bg)]',
          )}
        >
          {stacked && <ScrollEdge at="top" />}
          <div
            ref={leftRef}
            data-testid="project-settings-left"
            className={halves.left(stacked)}
          >
            {!stacked && <ScrollEdge at="top" inset="-mx-6" />}
            <div ref={generalRef} className="pt-5 pb-3">
              {general}
            </div>
            <DashboardsSection
              detail={detail}
              caps={caps}
              docs={docs}
              docsError={docsError}
              onReloadDocs={onReloadDocs}
              bar={bar}
            />
            {!stacked && <ScrollEdge at="bottom" inset="-mx-6" room />}
          </div>
          <div
            data-testid="project-settings-right"
            className={halves.right(stacked)}
          >
            {!stacked && <ScrollEdge at="top" inset="-mx-6" />}
            <div className={halves.rightTop(stacked)}>
              <AgentsSection detail={detail} caps={caps} edit={edit} />
            </div>
            {!stacked && <ScrollEdge at="bottom" inset="-mx-6" room />}
          </div>
          {stacked && <ScrollEdge at="bottom" />}
        </div>
      </div>
    </>
  )
}

// ── New project (the list's footer and the empty page) ───────────────────

function NewProjectDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void
  onCreated: (group: ProjectGroup) => void
}): React.JSX.Element {
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const fieldId = useId()

  const submit = async (): Promise<void> => {
    const trimmed = name.trim()
    if (!trimmed || busy) return
    setBusy(true)
    try {
      const group = await createProjectGroup(primaryScope(), trimmed)
      onCreated(group)
    } catch (err) {
      setError(createErrorMessage(err))
      setBusy(false)
    }
  }

  return (
    <AccessSheet
      title="New project"
      intro="A project puts agents together with one shared chat and its own dashboards. Add its agents once it exists."
      onClose={onClose}
      busy={busy}
      testId="new-project-dialog"
      footer={
        <>
            <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button variant="accent" size="md" disabled={busy || name.trim().length === 0} onClick={() => void submit()}>
              {busy ? 'Creating…' : 'Create project'}
            </Button>
        </>
      }
    >
      <Field label="Name" htmlFor={fieldId}>
        <Input
          id={fieldId}
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
          }}
          placeholder="Spring launch"
          className={DIALOG_FIELD}
        />
      </Field>
      {error && (
        <FailedNote what="Couldn’t create the project." next="Nothing was created. Change the name and try again." detail={error} />
      )}
    </AccessSheet>
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

/** A list section's heading (the Workspaces list's). */
function ListHeading({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <div className="flex items-center h-7 mb-1 type-heading text-[var(--color-text-primary)]">{children}</div>
}

/** One project's settings data, for the Settings page and the Projects
 *  page's sheet alike: the viewer's role, its show view (painted from the
 *  cache at once, revalidated on every structural change to it), its
 *  Workspace Resources, and the optimistic `edit`. `shown` is the selected
 *  project's view, else (while an unseen one loads) the previous one, with
 *  `switching` true. */
export function useProjectSettingsData(selectedId: string | null): {
  role: ProjectRole | null
  shown: ProjectGroupShow | null
  switching: boolean
  detailError: { id: string; message: string } | null
  docs: ProjectGroupHtmlDoc[] | null
  docsError: string | null
  edit: (groupId: string, patch: Partial<ProjectGroup>, run: () => Promise<ProjectGroup | void>) => Promise<void>
  reload: () => void
  reloadDocs: () => void
} {
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
  const [docsNonce, setDocsNonce] = useState(0)

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
        setDetailError({ id: selectedId, message: errorMessage(e) })
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
  }, [selectedId, revision, reloadNonce, docsNonce])

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

  // Optimistic edit (rename / pin / color / point of contact): apply now to
  // this view, the show cache and the store's list row; take the daemon's
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

  const reload = useCallback(() => setReloadNonce((n) => n + 1), [])
  const reloadDocs = useCallback(() => setDocsNonce((n) => n + 1), [])
  return {
    role,
    shown,
    switching,
    detailError,
    docs: shownDocs,
    docsError: shownDocsError,
    edit,
    reload,
    reloadDocs,
  }
}

export default function ProjectSettings(): React.JSX.Element {
  // The page's width in CSS px (any app zoom): narrow, the list goes on top.
  const [pageRef, pageWidth] = useCssWidth(PROJECTS_SPLIT_MIN_PX)
  const groups = useProjectGroupsStore((s) => s.groups)
  const groupsError = useProjectGroupsStore((s) => s.groupsError)

  // The section's OWN selection — seeded from the deep-link preselect
  // (initialProjectGroupId — the gear / right-click), else the Projects
  // page's nav selection, else the first group. Switching here never
  // moves the page's nav selection.
  const initialGroupId = useSettingsStore((s) => s.initialProjectGroupId)
  const [selectedId, setSelectedId] = useState<string | null>(
    () => initialGroupId ?? useProjectGroupsStore.getState().selectedGroupId,
  )
  const [creating, setCreating] = useState(false)
  /** What the page just did, shown once above the next project. */
  const [notice, setNotice] = useState<{ forId: string | null; text: string } | null>(null)

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

  const { role, shown, switching, detailError, docs: shownDocs, docsError: shownDocsError, edit, reload, reloadDocs } =
    useProjectSettingsData(selectedId)

  // ── Left master list: search + keyboard selection, Pinned first ─────
  const [searchQuery, setSearchQuery] = useState('')
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const { pinned: pinnedGroups, unpinned: otherGroups } = useMemo(
    () => partitionPinned(filterGroupsByQuery(groups ?? [], searchQuery)),
    [groups, searchQuery],
  )
  const visibleGroups = useMemo(() => [...pinnedGroups, ...otherGroups], [pinnedGroups, otherGroups])
  const hasList = groups !== null && groups.length > 0
  useEffect(() => {
    setKeyboardIndex(-1)
  }, [searchQuery])
  // While a search is showing, the right side follows it: the selected
  // project if it matches, else the first match (none: a "Pick a project"
  // panel, below).
  const searchActive = searchQuery.trim().length > 0
  useEffect(() => {
    if (!searchActive || visibleGroups.length === 0) return
    if (!visibleGroups.some((g) => g.id === selectedId)) setSelectedId(visibleGroups[0].id)
  }, [searchActive, visibleGroups, selectedId])
  useEffect(() => {
    if (!hasList) return
    requestAnimationFrame(() => searchInputRef.current?.focus())
  }, [hasList])

  const select = useCallback((id: string): void => {
    setSelectedId(id)
    setNotice((n) => (n && n.forId !== id ? null : n))
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
        select(visibleGroups[keyboardIndex].id)
      }
    },
    [visibleGroups, keyboardIndex, select],
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

  /** Arrow keys on a focused row move to the next or previous row. */
  const moveFromRow = (id: string, step: 1 | -1): void => {
    const i = visibleGroups.findIndex((g) => g.id === id)
    const next = visibleGroups[i + step]
    if (!next) return
    document.querySelector<HTMLElement>(`[data-settings-group-id="${next.id}"]`)?.focus()
  }

  const retryList = (): void => {
    void useProjectGroupsStore.getState().fetchGroups()
  }

  const onCreated = (group: ProjectGroup): void => {
    setCreating(false)
    setSearchQuery('')
    setNotice({ forId: group.id, text: `Created “${group.name}”. Add its agents on the right.` })
    setSelectedId(group.id)
    void useProjectGroupsStore.getState().fetchGroups()
  }

  const groupRow = (g: (typeof visibleGroups)[number]): React.JSX.Element => {
    const isSelected = selectedId === g.id
    const isKeyboardHighlighted = !isSelected && visibleGroups.indexOf(g) === keyboardIndex
    return (
      <div
        key={g.id}
        data-settings-group-id={g.id}
        data-selected={isSelected ? '' : undefined}
        role="button"
        tabIndex={0}
        data-k2-focus=""
        aria-label={`Show settings for ${g.name}`}
        aria-current={isSelected ? 'true' : undefined}
        onClick={() => select(g.id)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            select(g.id)
          } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault()
            moveFromRow(g.id, e.key === 'ArrowDown' ? 1 : -1)
          }
        }}
        className={cx(
          'relative flex items-center gap-3 px-3 py-2 transition-colors no-drag cursor-pointer select-none outline-none',
          isSelected
            ? 'bg-[var(--color-accent)]/15'
            : isKeyboardHighlighted
              ? SETTING_LIST_ROW_BAND
              : SETTING_LIST_ROW_HOVER,
        )}
      >
        {isSelected && (
          <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />
        )}
        <span className={AVATAR_ROUND}>
          <ProjectGroupAvatar name={g.name} groupId={g.id} size={24} color={g.color} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block type-name text-[var(--color-text-primary)] truncate">{g.name}</span>
          <span className="block type-hint text-[var(--color-text-secondary)] truncate">
            {g.memberCount} {g.memberCount === 1 ? 'agent' : 'agents'}
          </span>
        </span>
      </div>
    )
  }

  const zone = (rows: typeof visibleGroups): React.JSX.Element => (
    // overflow-hidden: a selected or hovered row keeps to the card's corners.
    <Surface chamfer material="tile" className="overflow-hidden">
      <div className="divide-y divide-[var(--color-border)]">{rows.map(groupRow)}</div>
    </Surface>
  )

  const newProjectDialog = creating ? (
    <NewProjectDialog onClose={() => setCreating(false)} onCreated={onCreated} />
  ) : null

  // ── One page (no list yet): loading, failed, or no projects ──────────
  if (!hasList) {
    return (
      <div className="h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
        {newProjectDialog}
        <div className="w-full max-w-[52rem] space-y-6 pb-6">
          <header className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h2 className="type-title text-[var(--color-text-primary)]">Projects</h2>
              <p className="type-hint text-[var(--color-text-secondary)] mt-1">
                A project puts agents together with one shared chat and its own dashboards.
              </p>
            </div>
          </header>
          {notice && (
            <Callout tone="ok" role="status" className="type-hint" data-testid="project-settings-notice">
              {notice.text}
            </Callout>
          )}
          {groups === null && groupsError ? (
            <SettingsLoadState
              kind="error"
              title="Couldn’t load your projects"
              detail={groupsError}
              testId="project-settings-list-error"
              action={<Button variant="quiet" onClick={retryList}>Try again</Button>}
            >
              {PRODUCT_NAME} couldn’t read the project list from this server, so nothing is shown here.
            </SettingsLoadState>
          ) : groups === null ? (
            <SettingsLoadState kind="loading" title="Loading your projects…" testId="project-settings-loading" />
          ) : (
            <SettingsLoadState
              kind="empty"
              title="No projects yet"
              testId="project-settings-empty"
              action={
                <Button variant="accent" onClick={() => setCreating(true)}>
                  New project
                </Button>
              }
            >
              Make one to give a few agents a shared chat and dashboards.
            </SettingsLoadState>
          )}
        </div>
      </div>
    )
  }

  const searching = searchActive
  const listOnTop = pageWidth < PROJECTS_SPLIT_MIN_PX
  const shownNotice = notice && shown && (notice.forId === null || notice.forId === shown.id) ? notice.text : null

  return (
    <div
      ref={pageRef}
      data-project-settings-layout={listOnTop ? 'stacked' : 'split'}
      className={listOnTop ? 'flex flex-col h-full min-h-0' : 'flex h-full min-h-0'}
    >
      {newProjectDialog}
      {/* ── Left (or, narrow, on top): the project list ── */}
      <aside
        aria-label="Projects"
        className={
          listOnTop
            ? 'max-h-[40%] flex-shrink-0 min-h-0 flex flex-col border-b border-[var(--color-border)]'
            : 'w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]'
        }
      >
        <div className="flex-shrink-0 px-4 pt-6 pb-4">
          <SearchField
            ref={searchInputRef}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onClear={() => setSearchQuery('')}
            onKeyDown={handleSearchKeyDown}
            placeholder="Search projects"
            aria-label="Search projects"
            className="w-full min-w-0"
          />
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-5 [scrollbar-gutter:stable]">
          {searching && visibleGroups.length === 0 && (
            <Surface chamfer material="tile">
              <div className="px-4 py-4 flex flex-col items-start gap-3" data-testid="projects-no-match">
                <div className="min-w-0">
                  <div className="type-row text-[var(--color-text-primary)] break-words">
                    No projects match &ldquo;{searchQuery.trim()}&rdquo;
                  </div>
                  <div className={SETTING_ROW_HINT}>Check the spelling, or clear the search to see them all.</div>
                </div>
                <Button variant="quiet" onClick={() => setSearchQuery('')}>
                  Clear search
                </Button>
              </div>
            </Surface>
          )}
          {pinnedGroups.length > 0 && (
            <section>
              <ListHeading>Pinned</ListHeading>
              {zone(pinnedGroups)}
            </section>
          )}
          {otherGroups.length > 0 && (
            <section>
              <ListHeading>{pinnedGroups.length > 0 ? 'All projects' : 'Projects'}</ListHeading>
              {zone(otherGroups)}
            </section>
          )}
        </div>
        {/* Solid, above the list, so rows scroll under it. */}
        <div className="relative z-[1] flex-shrink-0 px-4 py-3 border-t border-[var(--color-border)] bg-[var(--color-bg)] flex items-center gap-2">
          <Button
            variant="quiet"
            aria-label="New project"
            className="flex-1 min-w-0 text-center"
            onClick={() => setCreating(true)}
          >
            + Project
          </Button>
        </div>
      </aside>

      {/* ── Right: the selected project ── */}
      <div className="flex-1 min-h-0 min-w-0">
        {searching && visibleGroups.length === 0 ? (
          <div className="p-6">
            <SettingsLoadState kind="empty" title="Pick a project" testId="project-settings-pick">
              No project matches the search. Clear it to pick one.
            </SettingsLoadState>
          </div>
        ) : selectedId === null ? (
          <div className="p-6">
            <SettingsLoadState kind="loading" title="Loading the project…" />
          </div>
        ) : detailError && detailError.id === selectedId ? (
          <div className="p-6">
            <SettingsLoadState
              kind="error"
              title="Couldn’t load this project"
              detail={detailError.message}
              testId="project-settings-detail-error"
              action={<Button variant="quiet" onClick={reload}>Try again</Button>}
            >
              Its settings can’t be shown right now. The project itself is fine.
            </SettingsLoadState>
          </div>
        ) : shown === null ? (
          /* First load only — after that a switch keeps the previous
             project on screen (below) instead of blanking. */
          <div className="p-6">
            <SettingsLoadState kind="loading" title="Loading the project…" testId="project-settings-detail-loading" />
          </div>
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
              onReloadDocs={reloadDocs}
              notice={shownNotice}
              onDeleted={(name) => {
                // Select the first surviving sibling (null when none —
                // the empty state); refetch NOW so the stale row never
                // gets re-selected by the fallback effect.
                const remaining = (useProjectGroupsStore.getState().groups ?? []).filter(
                  (g) => g.id !== selectedId,
                )
                const next = remaining[0]?.id ?? null
                setNotice({ forId: next, text: `Deleted “${name}”. Its agents and their folders are still on this server.` })
                setSelectedId(next)
                void useProjectGroupsStore.getState().fetchGroups()
              }}
            />
          </div>
        )}
      </div>
    </div>
  )
}
