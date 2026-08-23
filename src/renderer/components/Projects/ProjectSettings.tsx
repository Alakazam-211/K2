// Projects V1 P8 (prd-projects-v1 §6.5) — the per-project Settings
// surface. RELOCATION directive (Rosson, 2026-07-05 live review): this
// lives INSIDE the core Settings area as its own "Projects" section
// (Settings.tsx, id 'project-groups' — NOT the legacy 'projects' id,
// which is the Workspaces section), so projects flow exactly like
// workspaces. Deep-links (the Projects-page header gear + the nav
// right-click) open Settings at this section with a project preselected
// via useSettingsStore.initialProjectGroupId.
//
// LAYOUT DIRECTIVE (Rosson, 2026-07-05): mirror the Workspaces settings
// page — Settings/sections/ProjectsSection.tsx's master-detail column
// viewer, restyled minimally. Left column = the selectable PROJECT list
// (search + ArrowUp/ArrowDown/Enter keyboard selection +
// scroll-into-view — its selectedProjectId idiom); right panel = the
// selected project's management surface. The left list can switch
// projects without touching the Projects page's nav selection.
//
// The right panel manages, per §6.5 (+§6.7.6):
//   - Project: rename + canonical nav pin (routes existed since P2).
//   - Dashboards: the SAME shape as the Projects page — a TAB per
//     dashboard (`position` order) + an add affordance; each tab's
//     panel manages THAT dashboard: rename (dashboard/rename), reorder
//     (left/right moves → dashboard/reorder with the full id order),
//     the pinned-HTML browser (GET html-docs, member workspaces only,
//     resolved Q3) whose "Add to <name>" appends an
//     {kind:"htmlDoc",workspaceId,filePath} column via the P5 layout
//     machinery + save-layout (project-settings.ts), and delete
//     (dashboard/delete) — refused for the LAST dashboard (button
//     disabled at one; the daemon 409 `last_dashboard` backstops).
//   - Members: add (picker over the registered-workspace list) /
//     remove, with the PoC-successor rule surfaced — removing the PoC
//     is DISABLED with the explanation until a successor is chosen
//     (daemon 409 poc_successor_required is the backstop).
//   - PoC: the reassignment dropdown (members only) → set-poc.
//   - Danger zone: delete project — removes the group, its member
//     rows, messages, and dashboards; NEVER the workspaces (locked
//     default, ledger §11).
//
// Permissions (§6.3b precedent): owners AND admins mutate; a resolved
// viewer-mode window (window-mode.ts — the P5/P6 idiom) is READ-ONLY;
// the daemon's owner-or-admin gate on the dashboard mutations
// backstops. Live: rides the store's project-group:* revision — every
// event refetches the selected project's show view + the docs list.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import {
  addProjectGroupMember,
  createErrorMessage,
  createProjectGroupDashboard,
  daemonErrorInfo,
  deleteProjectGroup,
  deleteProjectGroupDashboard,
  fetchProjectGroupResources,
  fetchProjectGroupIcon,
  fetchProjectGroupShow,
  normalizeHexColor,
  pinProjectGroup,
  removeProjectGroupMember,
  renameProjectGroup,
  renameProjectGroupDashboard,
  reorderProjectGroupDashboards,
  saveDashboardLayout,
  setProjectGroupColor,
  setProjectGroupIcon,
  setProjectGroupPoc,
  type ProjectGroupDashboard,
  type ProjectGroupHtmlDoc,
  type ProjectGroupShow,
} from './projects-api'
import { onWorkspaceResourcesChanged } from '@/stores/session-events'
import { pickIconImage } from '@/lib/pick-remote-image'
import { GROUP_AVATAR_COLORS, groupAvatarColor } from './ProjectGroupAvatar'
import IconCropDialog from '@/components/Settings/IconCropDialog'
import {
  addableWorkspaces,
  appendHtmlDocPane,
  filterGroupsByQuery,
  removeMemberBlockedReason,
} from './project-settings'
import { moveDashboardId, orderedDashboards } from './project-tabs'

/** Uniform section header (the ProjectsSection h3 idiom). */
function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
      {children}
    </h3>
  )
}

function errorMessage(err: unknown): string {
  const { hint } = daemonErrorInfo(err)
  if (hint) return hint
  return err instanceof Error ? err.message : String(err)
}

// ── Inline rename (project + dashboard rows share the idiom) ─────────────

function InlineRename({
  value,
  label,
  onSave,
}: {
  value: string
  label: string
  onSave: (name: string) => Promise<void>
}): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    // A canonical rename elsewhere (event refetch) wins over a stale
    // non-editing view; an open editor keeps its draft.
    if (!editing) setDraft(value)
  }, [value, editing])

  const commit = async (): Promise<void> => {
    const name = draft.trim()
    if (!name || name === value) {
      setEditing(false)
      setError(null)
      return
    }
    setSaving(true)
    try {
      await onSave(name)
      setEditing(false)
      setError(null)
    } catch (err) {
      setError(createErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  if (!editing) {
    return (
      <div className="flex items-center gap-2 min-w-0">
        <span className="text-xs text-[var(--color-text-primary)] truncate">{value}</span>
        <button
          type="button"
          onClick={() => {
            setEditing(true)
            setError(null)
            requestAnimationFrame(() => inputRef.current?.focus())
          }}
          className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-accent)] transition-colors cursor-pointer flex-shrink-0"
          title={`Rename ${label}`}
        >
          Rename
        </button>
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-1 min-w-0 flex-1">
      <div className="flex items-center gap-1.5">
        <input
          ref={inputRef}
          autoFocus
          type="text"
          value={draft}
          disabled={saving}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void commit()
            else if (e.key === 'Escape') {
              // Local Esc: cancel the rename WITHOUT closing Settings
              // (stopPropagation keeps it from Settings' window Esc).
              e.stopPropagation()
              setEditing(false)
              setDraft(value)
              setError(null)
            }
          }}
          className="flex-1 min-w-0 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
        />
        <button
          type="button"
          onClick={() => void commit()}
          disabled={saving}
          className="px-2 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50 flex-shrink-0"
        >
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
      {error && <p className="text-[10px] text-[var(--color-status-error-soft)]">{error}</p>}
    </div>
  )
}

// ── Dashboards block (§6.7.6 — a tab per dashboard + add) ────────────────

/** Inline "add dashboard" affordance: a + tab that expands into a name
 *  input; POSTs dashboard/create, surfaces `name_taken` inline, selects
 *  the new tab on success (the CreateProjectForm idiom, tab-shaped). */
function AddDashboardTab({
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
      const dashboard = await createProjectGroupDashboard(groupId, trimmed)
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
      <button
        type="button"
        onClick={() => setEditing(true)}
        className="flex items-center gap-1 px-2 py-1.5 text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer flex-shrink-0"
        title="Add dashboard"
      >
        <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
        </svg>
      </button>
    )
  }

  return (
    <div className="flex flex-col gap-1 flex-shrink-0">
      <div className="flex items-center gap-1">
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
            else if (e.key === 'Escape') {
              // Local Esc: cancel the add WITHOUT closing Settings.
              e.stopPropagation()
              setEditing(false)
              setName('')
              setError(null)
            }
          }}
          placeholder="Dashboard name…"
          className="w-32 px-2 py-1 text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
        />
        <button
          type="button"
          onClick={() => void submit()}
          disabled={busy || name.trim().length === 0}
          className="px-2 py-1 text-[10px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 transition-colors cursor-pointer disabled:opacity-50"
        >
          {busy ? 'Adding…' : 'Add'}
        </button>
      </div>
      {error && <p className="text-[10px] text-[var(--color-status-error-soft)]">{error}</p>}
    </div>
  )
}

/** §6.7.6 — the Dashboards block, the SAME shape as the Projects page's
 *  tab row: one tab per dashboard (`position` order) + an add
 *  affordance (owners/admins; viewers read-only). Each tab's panel
 *  manages THAT dashboard: rename, reorder (left/right moves — the
 *  reorder route takes the full id order), the pinned-HTML browser
 *  ("Add to this dashboard" targets it), and delete — refused for the
 *  last dashboard (button disabled at one; the daemon's 409
 *  `last_dashboard` is the backstop and surfaces as a toast). */
function DashboardsBlock({
  detail,
  readOnly,
  docs,
  docsError,
}: {
  detail: ProjectGroupShow
  readOnly: boolean
  docs: ProjectGroupHtmlDoc[] | null
  docsError: string | null
}): React.JSX.Element {
  const dashboards = useMemo(
    () => orderedDashboards(detail.dashboards),
    [detail.dashboards],
  )
  const orderedIds = useMemo(() => dashboards.map((d) => d.id), [dashboards])

  // The selected dashboard tab — heals to the first when the selection
  // was deleted elsewhere (event refetch replaces `detail`).
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const active = dashboards.find((d) => d.id === selectedId) ?? dashboards[0] ?? null

  const [busy, setBusy] = useState(false)
  // Per-doc transient outcome ("Added" flash / already-there note).
  const [docNote, setDocNote] = useState<{ key: string; note: string } | null>(null)

  const reorder = useCallback(
    async (dashboardId: string, direction: -1 | 1): Promise<void> => {
      const order = moveDashboardId(orderedIds, dashboardId, direction)
      if (order === null || busy) return
      setBusy(true)
      try {
        await reorderProjectGroupDashboards(detail.id, order)
        // groups-changed refetches `detail` with the new positions.
      } catch (err) {
        useToastStore.getState().addToast(`Reorder failed: ${errorMessage(err)}`, 'error')
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
        message:
          'This removes the dashboard and its layout. Sessions and workspaces are never touched.',
        confirmLabel: 'Delete dashboard',
        destructive: true,
      })
      if (!confirmed) return
      try {
        await deleteProjectGroupDashboard(detail.id, dashboard.id)
        setSelectedId(null) // heal to the first surviving tab
      } catch (err) {
        const { code } = daemonErrorInfo(err)
        useToastStore
          .getState()
          .addToast(
            code === 'last_dashboard'
              ? 'A project keeps at least one dashboard — this is the last one.'
              : `Delete failed: ${errorMessage(err)}`,
            'error',
          )
      }
    },
    [detail.id],
  )

  const addDocToDashboard = useCallback(
    async (doc: ProjectGroupHtmlDoc, dashboard: ProjectGroupDashboard): Promise<void> => {
      const key = `${doc.workspaceId}:${doc.filePath}`
      const { layoutJson, added } = appendHtmlDocPane(
        dashboard.layoutJson,
        detail.pocWorkspaceId,
        doc,
      )
      if (!added) {
        setDocNote({ key, note: `Already on ${dashboard.name}` })
        return
      }
      try {
        await saveDashboardLayout(detail.id, dashboard.id, layoutJson)
        setDocNote({ key, note: `Added to ${dashboard.name}` })
      } catch (err) {
        useToastStore
          .getState()
          .addToast(`Add to dashboard failed: ${errorMessage(err)}`, 'error')
      }
    },
    [detail.id, detail.pocWorkspaceId],
  )

  const activeIndex = active ? orderedIds.indexOf(active.id) : -1
  const lastOne = dashboards.length === 1

  return (
    <div className="space-y-2">
      <SectionTitle>Dashboards</SectionTitle>

      {/* The tab row — the page's dashboards-as-tabs shape (§6.7.6). */}
      <div className="flex items-center gap-1 border-b border-[var(--color-border)] overflow-x-auto">
        {dashboards.map((d) => (
          <button
            key={d.id}
            type="button"
            onClick={() => setSelectedId(d.id)}
            className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer whitespace-nowrap ${
              active?.id === d.id
                ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
            }`}
          >
            {d.name}
          </button>
        ))}
        {!readOnly && (
          <AddDashboardTab groupId={detail.id} onCreated={(d) => setSelectedId(d.id)} />
        )}
      </div>

      {active === null ? (
        <p className="text-[11px] text-[var(--color-text-muted)] italic">No dashboards.</p>
      ) : (
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {/* ── Name (rename) + reorder + revision ── */}
          <div className="flex items-center gap-3 px-3 py-2">
            {readOnly ? (
              <span className="text-xs text-[var(--color-text-primary)] truncate">
                {active.name}
              </span>
            ) : (
              <InlineRename
                value={active.name}
                label="dashboard"
                onSave={async (name) => {
                  await renameProjectGroupDashboard(detail.id, active.id, name)
                }}
              />
            )}
            <span className="flex-1" />
            {!readOnly && dashboards.length > 1 && (
              <div className="flex items-center gap-0.5 flex-shrink-0">
                <button
                  type="button"
                  disabled={busy || activeIndex <= 0}
                  onClick={() => void reorder(active.id, -1)}
                  className="flex h-5 w-5 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 disabled:cursor-not-allowed transition-colors cursor-pointer"
                  title="Move left"
                >
                  <svg width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="6 2 3 5 6 8" />
                  </svg>
                </button>
                <button
                  type="button"
                  disabled={busy || activeIndex >= dashboards.length - 1}
                  onClick={() => void reorder(active.id, 1)}
                  className="flex h-5 w-5 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 disabled:cursor-not-allowed transition-colors cursor-pointer"
                  title="Move right"
                >
                  <svg width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                    <polyline points="4 2 7 5 4 8" />
                  </svg>
                </button>
              </div>
            )}
            <span className="text-[9px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
              rev {active.revision}
            </span>
          </div>

          {/* ── Workspace Resources → THIS dashboard (HTML-only add) ── */}
          <div className="px-3 py-2 space-y-1.5">
            <p className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
              Resources
            </p>
            {docsError ? (
              <p className="text-[11px] text-[var(--color-status-error-soft)]">Failed to load resources: {docsError}</p>
            ) : docs === null ? (
              <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>
            ) : docs.length === 0 ? (
              <p className="text-[11px] text-[var(--color-text-muted)] opacity-70">
                No resources yet — add files from a member workspace&apos;s Files tree.
              </p>
            ) : (
              <div className="divide-y divide-[var(--color-border)]">
                {docs.map((doc) => {
                  const key = `${doc.workspaceId}:${doc.filePath}`
                  const html = /\.html?$/i.test(doc.filePath)
                  return (
                    <div key={key} className="flex items-center gap-2 py-1.5 min-w-0">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 min-w-0">
                          <span className="text-xs text-[var(--color-text-primary)] truncate">
                            {doc.fileName}
                          </span>
                          <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                            {doc.agentName ?? doc.workspaceName ?? ''}
                          </span>
                          {doc.missing ? (
                            <span className="text-[9px] uppercase tracking-wide text-[var(--color-status-warn-soft)] flex-shrink-0">
                              missing
                            </span>
                          ) : null}
                        </div>
                        <p
                          className="text-[10px] text-[var(--color-text-muted)] truncate"
                          title={doc.filePath}
                        >
                          {doc.filePath}
                        </p>
                      </div>
                      {docNote?.key === key && (
                        <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0">
                          {docNote.note}
                        </span>
                      )}
                      {!readOnly && html && (
                        <button
                          type="button"
                          onClick={() => void addDocToDashboard(doc, active)}
                          className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer flex-shrink-0"
                        >
                          Add to {active.name}
                        </button>
                      )}
                    </div>
                  )
                })}
              </div>
            )}
          </div>

          {/* ── Delete THIS dashboard ── */}
          {!readOnly && (
            <div className="flex items-center gap-3 px-3 py-2">
              <p className="flex-1 text-[10px] text-[var(--color-text-muted)]">
                Deleting removes this dashboard and its layout — never sessions or workspaces.
              </p>
              <button
                type="button"
                disabled={lastOne}
                onClick={() => void deleteDashboard(active)}
                title={
                  lastOne
                    ? 'A project keeps at least one dashboard.'
                    : `Delete "${active.name}"`
                }
                className={`px-2 py-0.5 text-[10px] flex-shrink-0 transition-colors ${
                  lastOne
                    ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                    : 'text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] cursor-pointer'
                }`}
              >
                Delete dashboard
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// ── The right panel (selected project's management) ──────────────────────

function ProjectSettingsDetail({
  detail,
  readOnly,
  onDeleted,
}: {
  detail: ProjectGroupShow
  readOnly: boolean
  onDeleted: () => void
}): React.JSX.Element {
  const registered = useProjectsStore((s) => s.projects)
  const revision = useProjectGroupsStore((s) => s.revision)

  // ── Workspace Resources picker (Add to dashboard is HTML-only) ─────
  const [docs, setDocs] = useState<ProjectGroupHtmlDoc[] | null>(null)
  const [docsError, setDocsError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    const load = (): void => {
      fetchProjectGroupResources(detail.id)
        .then((d) => {
          if (cancelled) return
          setDocs(d)
          setDocsError(null)
        })
        .catch((e) => {
          if (cancelled) return
          setDocsError(errorMessage(e))
        })
    }
    load()
    const off = onWorkspaceResourcesChanged(() => load())
    return () => {
      cancelled = true
      off()
    }
  }, [detail.id, revision])

  // ── §6.7.7 Icon + color ──────────────────────────────────────────────
  // The icon rides its own GET (not in show payloads); refetch on the
  // store revision so another client's upload lands here live (set-icon
  // emits groups-changed). null = still loading (controls suspend).
  const [icon, setIcon] = useState<{ found: boolean; dataUrl: string | null } | null>(null)
  const [iconBusy, setIconBusy] = useState(false)
  const [cropImage, setCropImage] = useState<string | null>(null)
  const iconInputRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    let cancelled = false
    fetchProjectGroupIcon(detail.id)
      .then((res) => {
        if (!cancelled) setIcon(res)
      })
      .catch(() => {
        // Advisory decoration — fall back to "no icon" controls.
        if (!cancelled) setIcon({ found: false, dataUrl: null })
      })
    return () => {
      cancelled = true
    }
  }, [detail.id, revision])

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
        await setProjectGroupIcon(detail.id, dataUrl)
        // Optimistic — the groups-changed refetch confirms it.
        setIcon({ found: dataUrl !== null, dataUrl })
      } catch (err) {
        useToastStore.getState().addToast(`Icon save failed: ${errorMessage(err)}`, 'error')
      } finally {
        setIconBusy(false)
      }
    },
    [detail.id],
  )

  // Custom-hex color input (§6.7.7 — the 10-swatch palette + a free
  // `#rrggbb` field; Enter applies, invalid shows inline).
  const [customHex, setCustomHex] = useState('')
  const [hexError, setHexError] = useState(false)

  const saveColor = useCallback(
    async (color: string | null): Promise<void> => {
      try {
        await setProjectGroupColor(detail.id, color)
        // groups-changed refetches the list + show with the new color.
      } catch (err) {
        useToastStore.getState().addToast(`Color change failed: ${errorMessage(err)}`, 'error')
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

  // ── Members: add picker + remove ─────────────────────────────────────
  const [adding, setAdding] = useState(false)
  const [addQuery, setAddQuery] = useState('')
  const [memberBusy, setMemberBusy] = useState<string | null>(null)
  const memberIds = useMemo(
    () => detail.members.map((m) => m.workspaceId),
    [detail.members],
  )
  const candidates = useMemo(
    () => addableWorkspaces(registered, memberIds, addQuery),
    [registered, memberIds, addQuery],
  )

  const addMember = useCallback(
    async (workspaceId: string): Promise<void> => {
      setMemberBusy(workspaceId)
      try {
        await addProjectGroupMember(detail.id, workspaceId)
        setAddQuery('')
        // The members-changed event refetches the show view live.
      } catch (err) {
        useToastStore.getState().addToast(`Add member failed: ${errorMessage(err)}`, 'error')
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
        await removeProjectGroupMember(detail.id, workspaceId)
      } catch (err) {
        // The daemon backstop (409 poc_successor_required) or a
        // vanished row — loud either way.
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
    async (workspaceId: string): Promise<void> => {
      try {
        await setProjectGroupPoc(detail.id, workspaceId)
      } catch (err) {
        useToastStore.getState().addToast(`PoC change failed: ${errorMessage(err)}`, 'error')
      }
    },
    [detail.id],
  )

  const deleteProject = useCallback(async (): Promise<void> => {
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Delete project "${detail.name}"?`,
      message:
        'This removes the project, its member list, chat messages, and dashboards. ' +
        'The workspaces themselves are never touched.',
      confirmLabel: 'Delete project',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await deleteProjectGroup(detail.id)
      onDeleted()
    } catch (err) {
      useToastStore.getState().addToast(`Delete failed: ${errorMessage(err)}`, 'error')
    }
  }, [detail.id, detail.name, onDeleted])

  const memberLabel = (workspaceId: string): string => {
    const m = detail.members.find((x) => x.workspaceId === workspaceId)
    return m?.agentName ?? m?.name ?? workspaceId.slice(0, 8)
  }

  // The group's effective avatar color — canonical when set, else the
  // stable hashed pick (what the nav renders without an override).
  const effectiveColor = detail.color ?? groupAvatarColor(detail.id)
  const firstLetter = (detail.name.trim()[0] ?? '?').toUpperCase()

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
    <div className="grid gap-6 grid-cols-[minmax(0,42rem)]">
      {/* ── Header ── */}
      <div className="min-w-0">
        <h2 className="text-base font-medium text-[var(--color-text-primary)] truncate">
          {detail.name}
        </h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          {detail.memberCount} {detail.memberCount === 1 ? 'member' : 'members'}
          {detail.pocWorkspaceId ? ` · PoC ${memberLabel(detail.pocWorkspaceId)}` : ' · no PoC yet'}
          {readOnly ? ' · view-only' : ''}
        </p>
      </div>

      {/* ── Project (rename + canonical pin) ── */}
      <div className="space-y-2">
        <SectionTitle>Project</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          <div className="flex items-center gap-3 px-3 py-2">
            <span className="text-[11px] text-[var(--color-text-secondary)] w-20 flex-shrink-0">
              Name
            </span>
            {readOnly ? (
              <span className="text-xs text-[var(--color-text-primary)] truncate">
                {detail.name}
              </span>
            ) : (
              <InlineRename
                value={detail.name}
                label="project"
                onSave={async (name) => {
                  await renameProjectGroup(detail.id, name)
                }}
              />
            )}
          </div>
          {/* ── Icon (§6.7.7 — the workspace uploader idiom: preview +
              file picker → crop/downscale → set-icon; Remove clears) ── */}
          <div className="flex items-center gap-3 px-3 py-2">
            <span className="text-[11px] text-[var(--color-text-secondary)] w-20 flex-shrink-0">
              Icon
            </span>
            <div
              className="flex-shrink-0 flex items-center justify-center overflow-hidden"
              style={{
                width: 48,
                height: 48,
                backgroundColor: icon?.dataUrl ? 'transparent' : effectiveColor,
                border: icon?.dataUrl ? `2px solid ${effectiveColor}` : 'none',
              }}
            >
              {icon?.dataUrl ? (
                <img
                  src={icon.dataUrl}
                  alt={detail.name}
                  style={{ width: '100%', height: '100%', objectFit: 'cover', objectPosition: 'center', display: 'block' }}
                />
              ) : (
                <span className="text-white font-bold" style={{ fontSize: 22, lineHeight: 1 }}>
                  {firstLetter}
                </span>
              )}
            </div>
            {!readOnly && (
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() =>
                    // Host-aware: on a remote host the native input would
                    // browse THIS machine's disk — use the remote picker.
                    void pickIconImage({
                      clickNativeInput: () => iconInputRef.current?.click(),
                      setCropImage,
                    })
                  }
                  disabled={iconBusy || icon === null}
                  className="px-2.5 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                >
                  {iconBusy ? 'Working...' : 'Upload'}
                </button>
                <input
                  ref={iconInputRef}
                  type="file"
                  accept="image/png,image/jpeg,image/svg+xml,image/x-icon"
                  className="hidden"
                  onChange={handleIconFileSelected}
                />
                {icon?.dataUrl && (
                  <button
                    type="button"
                    onClick={() => void saveIcon(null)}
                    disabled={iconBusy}
                    className="px-2.5 py-1 text-xs text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                  >
                    Remove
                  </button>
                )}
              </div>
            )}
          </div>
          {/* ── Color (§6.7.7 — the avatar palette as swatches + Auto +
              a custom #rrggbb field; canonical, everyone sees it) ── */}
          <div className="flex items-center gap-3 px-3 py-2">
            <span className="text-[11px] text-[var(--color-text-secondary)] w-20 flex-shrink-0">
              Color
            </span>
            {readOnly ? (
              <span className="flex items-center gap-2 text-xs text-[var(--color-text-primary)]">
                <span
                  className="w-4 h-4 flex-shrink-0 inline-block"
                  style={{ backgroundColor: effectiveColor }}
                />
                {detail.color ?? 'Auto'}
              </span>
            ) : (
              <div className="flex items-center gap-1.5 flex-wrap min-w-0">
                {/* Auto — clear back to the stable hashed pick. */}
                <button
                  type="button"
                  onClick={() => void saveColor(null)}
                  title="Auto — the stable per-project color"
                  className={`w-4 h-4 flex-shrink-0 no-drag cursor-pointer transition-transform text-[8px] font-bold leading-none text-white/80 ${
                    detail.color === null ? 'scale-125 ring-1 ring-white/50' : 'hover:scale-110'
                  }`}
                  style={{ backgroundColor: groupAvatarColor(detail.id) }}
                >
                  A
                </button>
                {GROUP_AVATAR_COLORS.map((color) => (
                  <button
                    key={color}
                    type="button"
                    onClick={() => void saveColor(color)}
                    className={`w-4 h-4 flex-shrink-0 no-drag cursor-pointer transition-transform ${
                      detail.color === color ? 'scale-125 ring-1 ring-white/50' : 'hover:scale-110'
                    }`}
                    style={{ backgroundColor: color }}
                  />
                ))}
                <input
                  type="text"
                  value={customHex}
                  onChange={(e) => {
                    setCustomHex(e.target.value)
                    setHexError(false)
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commitCustomHex()
                    else if (e.key === 'Escape') {
                      // Local Esc: clear the field WITHOUT closing Settings.
                      e.stopPropagation()
                      setCustomHex('')
                      setHexError(false)
                    }
                  }}
                  onBlur={() => commitCustomHex()}
                  placeholder="#rrggbb"
                  className={`w-20 px-2 py-1 text-[11px] font-mono bg-[var(--color-bg)] border text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none no-drag ${
                    hexError
                      ? 'border-[color-mix(in_srgb,var(--color-status-error-soft)_60%,transparent)]'
                      : 'border-[var(--color-border)] focus:border-[var(--color-accent)]'
                  }`}
                />
                {hexError && (
                  <span className="text-[10px] text-[var(--color-status-error-soft)]">Use #rrggbb</span>
                )}
              </div>
            )}
          </div>
          <div className="flex items-center gap-3 px-3 py-2">
            <span className="text-[11px] text-[var(--color-text-secondary)] w-20 flex-shrink-0">
              Pinned
            </span>
            <button
              type="button"
              disabled={readOnly}
              onClick={() => {
                void pinProjectGroup(detail.id, !detail.pinned).catch((err) =>
                  useToastStore.getState().addToast(`Pin failed: ${errorMessage(err)}`, 'error'),
                )
              }}
              className={`w-7 h-3.5 flex items-center transition-colors no-drag flex-shrink-0 ${
                detail.pinned ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
              } ${readOnly ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer'}`}
              title="Show in the nav's Pinned section (canonical — everyone sees it)"
            >
              <span
                className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
                  detail.pinned ? 'translate-x-3.5' : 'translate-x-0.5'
                }`}
              />
            </button>
          </div>
        </div>
      </div>

      {/* ── Dashboards (§6.7.6 — the SAME shape as the page: a tab per
          dashboard + an add affordance; each tab's panel manages THAT
          dashboard) ── */}
      <DashboardsBlock detail={detail} readOnly={readOnly} docs={docs} docsError={docsError} />

      {/* ── Members ── */}
      <div className="space-y-2">
        <SectionTitle>Members</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {detail.members.map((m) => {
            const isPoc = m.workspaceId === detail.pocWorkspaceId
            const blocked = removeMemberBlockedReason(m.workspaceId, detail.pocWorkspaceId)
            const display = m.agentName ?? m.name ?? m.workspaceId.slice(0, 8)
            return (
              <div key={m.workspaceId} className="flex items-center gap-2 px-3 py-2 min-w-0">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 min-w-0">
                    <span className="text-xs text-[var(--color-text-primary)] truncate">
                      {display}
                    </span>
                    {isPoc && (
                      <span className="text-[9px] font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)] flex-shrink-0">
                        PoC
                      </span>
                    )}
                  </div>
                  {m.path && (
                    <p
                      className="text-[10px] text-[var(--color-text-muted)] truncate"
                      title={m.path}
                    >
                      {m.path}
                    </p>
                  )}
                </div>
                {!readOnly && (
                  <button
                    type="button"
                    disabled={blocked !== null || memberBusy === m.workspaceId}
                    onClick={() => void removeMember(m.workspaceId, display)}
                    title={blocked ?? 'Remove from this project (the workspace itself is untouched)'}
                    className={`px-2 py-0.5 text-[10px] flex-shrink-0 transition-colors ${
                      blocked !== null
                        ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                        : 'text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] cursor-pointer'
                    }`}
                  >
                    {memberBusy === m.workspaceId ? 'Removing…' : 'Remove'}
                  </button>
                )}
              </div>
            )
          })}
          {detail.members.length === 0 && (
            <div className="px-3 py-2 text-[11px] text-[var(--color-text-muted)] italic">
              No members yet — the first one added becomes the Point of Contact.
            </div>
          )}
        </div>
        {detail.pocWorkspaceId !== null && detail.members.length > 0 && !readOnly && (
          <p className="text-[10px] text-[var(--color-text-muted)] opacity-70">
            The Point of Contact can&rsquo;t be removed until a successor is chosen below.
          </p>
        )}

        {/* Add member — picker over the registered-workspace list. */}
        {!readOnly &&
          (adding ? (
            <div className="border border-[var(--color-border)]">
              <input
                autoFocus
                type="text"
                value={addQuery}
                onChange={(e) => setAddQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') {
                    e.stopPropagation()
                    setAdding(false)
                    setAddQuery('')
                  } else if (e.key === 'Enter' && candidates.length === 1) {
                    void addMember(candidates[0].id).then(() => setAdding(false))
                  }
                }}
                placeholder="Search workspaces…"
                className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border-b border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none no-drag"
              />
              <div className="max-h-48 overflow-y-auto">
                {candidates.map((w) => (
                  <button
                    key={w.id}
                    type="button"
                    disabled={memberBusy === w.id}
                    onClick={() => void addMember(w.id)}
                    className="w-full flex items-center gap-2 px-2 py-1.5 text-left text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer disabled:opacity-50"
                  >
                    <span className="text-xs truncate">{w.name}</span>
                    <span className="text-[10px] text-[var(--color-text-muted)] truncate flex-1">
                      {w.path}
                    </span>
                    <span className="text-[10px] text-[var(--color-accent)] flex-shrink-0">
                      {memberBusy === w.id ? 'Adding…' : 'Add'}
                    </span>
                  </button>
                ))}
                {candidates.length === 0 && (
                  <div className="px-2 py-2 text-[11px] text-[var(--color-text-muted)] italic">
                    {registered.length === memberIds.length
                      ? 'Every registered workspace is already a member.'
                      : 'No workspaces match.'}
                  </div>
                )}
              </div>
              <button
                type="button"
                onClick={() => {
                  setAdding(false)
                  setAddQuery('')
                }}
                className="w-full px-2 py-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border-t border-[var(--color-border)] transition-colors cursor-pointer"
              >
                Done
              </button>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => setAdding(true)}
              className="w-full flex items-center justify-center gap-1.5 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] bg-white/[0.04] hover:bg-white/[0.08] transition-colors no-drag cursor-pointer"
            >
              <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
              </svg>
              Add workspace
            </button>
          ))}
      </div>

      {/* ── Point of Contact ── */}
      <div className="space-y-2">
        <SectionTitle>Point of Contact</SectionTitle>
        <select
          value={detail.pocWorkspaceId ?? ''}
          disabled={readOnly || detail.members.length === 0}
          onChange={(e) => {
            if (e.target.value) void setPoc(e.target.value)
          }}
          className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] focus:outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {detail.members.length === 0 && <option value="">No members yet</option>}
          {detail.members.map((m) => (
            <option key={m.workspaceId} value={m.workspaceId}>
              {m.agentName ?? m.name ?? m.workspaceId.slice(0, 8)}
            </option>
          ))}
        </select>
        <p className="text-[10px] text-[var(--color-text-muted)] opacity-70">
          Every project chat message (except the PoC&rsquo;s own) is injected into the
          PoC&rsquo;s session.
        </p>
      </div>

      {/* ── Danger zone ── */}
      {!readOnly && (
        <div className="space-y-2 pb-6">
          <SectionTitle>Danger zone</SectionTitle>
          <div className="border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] px-3 py-2 flex items-center gap-3">
            <div className="flex-1 min-w-0">
              <p className="text-xs text-[var(--color-text-primary)]">Delete this project</p>
              <p className="text-[10px] text-[var(--color-text-muted)]">
                Removes the project, its member list, chat messages, and dashboards. The
                workspaces themselves are never touched.
              </p>
            </div>
            <button
              type="button"
              onClick={() => void deleteProject()}
              className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] transition-colors cursor-pointer flex-shrink-0"
            >
              Delete project
            </button>
          </div>
        </div>
      )}
    </div>
    </>
  )
}

// ── The settings surface (master-detail) ─────────────────────────────────

export default function ProjectSettings(): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const revision = useProjectGroupsStore((s) => s.revision)
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')

  // The section's OWN selection — seeded from the deep-link preselect
  // (initialProjectGroupId — the gear / right-click), else the Projects
  // page's nav selection, else the first group. Switching here never
  // moves the page's nav selection.
  const initialGroupId = useSettingsStore((s) => s.initialProjectGroupId)
  const [selectedId, setSelectedId] = useState<string | null>(
    () => initialGroupId ?? useProjectGroupsStore.getState().selectedGroupId,
  )
  const [detail, setDetail] = useState<ProjectGroupShow | null>(null)
  const [detailError, setDetailError] = useState<string | null>(null)

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
  // project-group event (the store's revision — groups/members/poc/
  // layout changed all land here; the store already coalesces).
  useEffect(() => {
    if (selectedId === null) {
      setDetail(null)
      setDetailError(null)
      return
    }
    let cancelled = false
    fetchProjectGroupShow(selectedId)
      .then((data) => {
        if (cancelled) return
        setDetail(data)
        setDetailError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDetailError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [selectedId, revision])

  // ── Left master list: search + keyboard selection (ProjectsSection
  //    idiom) ──────────────────────────────────────────────────────────
  const [searchQuery, setSearchQuery] = useState('')
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const visibleGroups = useMemo(
    () => filterGroupsByQuery(groups ?? [], searchQuery),
    [groups, searchQuery],
  )
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

  return (
    /* Master-detail (§6.5 layout directive — the ProjectsSection column
       viewer: left selectable list, right selected details). h-full: the
       Settings content area renders this section p-0/overflow-hidden so
       both columns stretch (the Workspaces-section idiom). */
    <div className="flex h-full min-h-0">
      {/* ── Left: the project list ── */}
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col">
        <div className="px-2 py-1.5">
          <input
            ref={searchInputRef}
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleSearchKeyDown}
            placeholder="Search projects..."
            className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
          />
        </div>
        <div className="flex-1 overflow-y-auto px-1 py-1">
          {visibleGroups.map((g) => {
            const isSelected = selectedId === g.id
            const kbIdx = visibleGroups.findIndex((vg) => vg.id === g.id)
            const isKeyboardHighlighted = kbIdx >= 0 && kbIdx === keyboardIndex
            return (
              <div
                key={g.id}
                data-settings-group-id={g.id}
                onClick={() => setSelectedId(g.id)}
                className={`flex items-center gap-2 px-2 py-1.5 transition-colors no-drag cursor-pointer select-none ${
                  isSelected
                    ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                    : isKeyboardHighlighted
                      ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
                }`}
              >
                <span
                  className={`flex items-center justify-center flex-shrink-0 w-5 h-5 text-[11px] font-semibold ${
                    isSelected
                      ? 'bg-[var(--color-accent)]/20 text-[var(--color-accent)]'
                      : 'bg-white/[0.06] text-[var(--color-text-secondary)]'
                  }`}
                >
                  {(g.name.trim()[0] ?? '?').toUpperCase()}
                </span>
                <span className="text-xs truncate flex-1">{g.name}</span>
                <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                  {g.memberCount}
                </span>
              </div>
            )
          })}
          {visibleGroups.length === 0 && (
            <div className="px-2 py-6 text-center">
              <span className="text-xs text-[var(--color-text-muted)]">
                {searchQuery.trim() ? 'No projects match' : 'No projects'}
              </span>
            </div>
          )}
        </div>
      </div>

      {/* ── Right: the selected project's management ── */}
      <div className="flex-1 overflow-y-auto p-6 min-h-0">
        {groups !== null && groups.length === 0 ? (
          <p className="text-xs text-[var(--color-text-muted)]">
            No projects yet — create one on the Projects page.
          </p>
        ) : selectedId === null || groups === null ? (
          <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
        ) : detailError ? (
          <p className="text-[11px] text-[var(--color-status-error-soft)]">Failed to load project: {detailError}</p>
        ) : detail === null || detail.id !== selectedId ? (
          /* Selection-switch staleness guard (the page idiom): never
             render the PREVIOUS project's data under a new title. */
          <p className="text-xs text-[var(--color-text-muted)]">Loading…</p>
        ) : (
          <ProjectSettingsDetail
            key={detail.id}
            detail={detail}
            readOnly={readOnly}
            onDeleted={() => {
              // Select the first surviving sibling (null when none —
              // the empty state); refetch NOW so the stale row never
              // gets re-selected by the fallback effect (the nav's
              // togglePin skip-the-coalesce idiom).
              const remaining = (useProjectGroupsStore.getState().groups ?? []).filter(
                (g) => g.id !== selectedId,
              )
              setSelectedId(remaining[0]?.id ?? null)
              void useProjectGroupsStore.getState().fetchGroups()
            }}
          />
        )}
      </div>
    </div>
  )
}
