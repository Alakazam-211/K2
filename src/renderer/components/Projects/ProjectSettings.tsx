// Projects V1 P8 (prd-projects-v1 §6.5) — the per-project Settings
// surface, replacing P4's SettingsPlaceholder.
//
// LAYOUT DIRECTIVE (Rosson, 2026-07-05): mirror the Workspaces settings
// page — Settings/sections/ProjectsSection.tsx's master-detail column
// viewer, restyled minimally. Left column = the selectable PROJECT list
// (search + ArrowUp/ArrowDown/Enter keyboard selection +
// scroll-into-view — its selectedProjectId idiom); right panel = the
// selected project's management surface. The gear deep-links here with
// the current project preselected (the `show` prop); the left list can
// switch projects without touching the page's nav selection.
//
// The right panel manages, per §6.5:
//   - Project: rename + canonical nav pin (routes existed since P2).
//   - Dashboards: the project's dashboard rows (V1 = the single 'Main')
//     with rename (the new dashboard/rename route); create/delete/
//     reorder are V1.1 and land HERE (the seam below).
//   - Members: add (picker over the registered-workspace list) /
//     remove, with the PoC-successor rule surfaced — removing the PoC
//     is DISABLED with the explanation until a successor is chosen
//     (daemon 409 poc_successor_required is the backstop).
//   - PoC: the reassignment dropdown (members only) → set-poc.
//   - Pinned HTML pages: the new GET html-docs route's rows (member
//     workspaces only, resolved Q3) with "Add to dashboard" — appends
//     an {kind:"htmlDoc",workspaceId,filePath} column via the P5
//     layout machinery + save-layout (project-settings.ts).
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
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import {
  addProjectGroupMember,
  createErrorMessage,
  daemonErrorInfo,
  deleteProjectGroup,
  fetchProjectGroupHtmlDocs,
  fetchProjectGroupShow,
  pinProjectGroup,
  removeProjectGroupMember,
  renameProjectGroup,
  renameProjectGroupDashboard,
  saveDashboardLayout,
  setProjectGroupPoc,
  type ProjectGroupHtmlDoc,
  type ProjectGroupShow,
} from './projects-api'
import {
  addableWorkspaces,
  appendHtmlDocColumn,
  filterGroupsByQuery,
  removeMemberBlockedReason,
} from './project-settings'

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
              // (the panel's capture handler skips editable targets).
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
      {error && <p className="text-[10px] text-red-400">{error}</p>}
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

  // ── Pinned-HTML browser data (the new html-docs route) ──────────────
  const [docs, setDocs] = useState<ProjectGroupHtmlDoc[] | null>(null)
  const [docsError, setDocsError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    fetchProjectGroupHtmlDocs(detail.id)
      .then((d) => {
        if (cancelled) return
        setDocs(d)
        setDocsError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setDocsError(errorMessage(e))
      })
    return () => {
      cancelled = true
    }
  }, [detail.id, revision])

  // "Add to dashboard" target — V1 shows the single 'Main' row; the
  // select is already dashboard-id-addressed for V1.1.
  const [targetDashboardId, setTargetDashboardId] = useState<string>(
    detail.dashboards[0]?.id ?? '',
  )
  useEffect(() => {
    if (!detail.dashboards.some((d) => d.id === targetDashboardId)) {
      setTargetDashboardId(detail.dashboards[0]?.id ?? '')
    }
  }, [detail.dashboards, targetDashboardId])

  // Per-doc transient outcome ("Added" flash / already-there note).
  const [docNote, setDocNote] = useState<{ key: string; note: string } | null>(null)

  const addDocToDashboard = useCallback(
    async (doc: ProjectGroupHtmlDoc): Promise<void> => {
      const dashboard = detail.dashboards.find((d) => d.id === targetDashboardId)
      if (!dashboard) return
      const key = `${doc.workspaceId}:${doc.filePath}`
      const { layoutJson, added } = appendHtmlDocColumn(
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
    [detail.id, detail.dashboards, detail.pocWorkspaceId, targetDashboardId],
  )

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

  return (
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
                className={`w-2.5 h-2.5 bg-white block transition-transform ${
                  detail.pinned ? 'translate-x-3.5' : 'translate-x-0.5'
                }`}
              />
            </button>
          </div>
        </div>
      </div>

      {/* ── Dashboards (V1: the single 'Main' row, rename allowed) ── */}
      <div className="space-y-2">
        <SectionTitle>Dashboards</SectionTitle>
        <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
          {detail.dashboards.map((d) => (
            <div key={d.id} className="flex items-center gap-3 px-3 py-2">
              {readOnly ? (
                <span className="text-xs text-[var(--color-text-primary)] truncate">{d.name}</span>
              ) : (
                <InlineRename
                  value={d.name}
                  label="dashboard"
                  onSave={async (name) => {
                    await renameProjectGroupDashboard(detail.id, d.id, name)
                  }}
                />
              )}
              <span className="flex-1" />
              <span className="text-[9px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                rev {d.revision}
              </span>
            </div>
          ))}
          {detail.dashboards.length === 0 && (
            <div className="px-3 py-2 text-[11px] text-[var(--color-text-muted)] italic">
              No dashboards.
            </div>
          )}
        </div>
        {/* V1.1 seam: create / delete / reorder land HERE — the routes
            and schema are dashboard-id-addressed already (§4.1). */}
        <p className="text-[10px] text-[var(--color-text-muted)] opacity-70">
          Creating, deleting, and reordering dashboards arrives in V1.1.
        </p>
      </div>

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
                        : 'text-red-400 border border-red-400/30 hover:bg-red-400/10 cursor-pointer'
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

      {/* ── Pinned HTML pages (the html-docs browser) ── */}
      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <SectionTitle>Pinned HTML pages</SectionTitle>
          <span className="flex-1" />
          {detail.dashboards.length > 1 && (
            <select
              value={targetDashboardId}
              onChange={(e) => setTargetDashboardId(e.target.value)}
              className="px-1.5 py-0.5 text-[10px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-secondary)] focus:outline-none no-drag cursor-pointer"
              title="Target dashboard"
            >
              {detail.dashboards.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </select>
          )}
        </div>
        {docsError ? (
          <p className="text-[11px] text-red-400">Failed to load pinned pages: {docsError}</p>
        ) : docs === null ? (
          <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>
        ) : docs.length === 0 ? (
          <p className="text-[11px] text-[var(--color-text-muted)] opacity-70">
            No pinned HTML pages — members pin .html files as tabs in their workspaces, and
            they show up here.
          </p>
        ) : (
          <div className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
            {docs.map((doc) => {
              const key = `${doc.workspaceId}:${doc.filePath}`
              return (
                <div key={key} className="flex items-center gap-2 px-3 py-2 min-w-0">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 min-w-0">
                      <span className="text-xs text-[var(--color-text-primary)] truncate">
                        {doc.fileName}
                      </span>
                      <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                        {doc.agentName ?? doc.workspaceName ?? ''}
                      </span>
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
                  {!readOnly && detail.dashboards.length > 0 && (
                    <button
                      type="button"
                      onClick={() => void addDocToDashboard(doc)}
                      className="px-2 py-0.5 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer flex-shrink-0"
                    >
                      Add to dashboard
                    </button>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>

      {/* ── Danger zone ── */}
      {!readOnly && (
        <div className="space-y-2 pb-6">
          <SectionTitle>Danger zone</SectionTitle>
          <div className="border border-red-400/30 px-3 py-2 flex items-center gap-3">
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
              className="px-2.5 py-1 text-[10px] font-medium text-red-400 border border-red-400/30 hover:bg-red-400/10 transition-colors cursor-pointer flex-shrink-0"
            >
              Delete project
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

// ── The settings surface (master-detail) ─────────────────────────────────

export default function ProjectSettings({
  show,
  onClose,
}: {
  /** The page's currently-open project — the gear's deep-link
   *  preselection (§6.5); the left list can switch away from it. */
  show: ProjectGroupShow
  onClose: () => void
}): React.JSX.Element {
  const groups = useProjectGroupsStore((s) => s.groups)
  const revision = useProjectGroupsStore((s) => s.revision)
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')

  // The settings surface's OWN selection (initialized to the open
  // project) — switching here never moves the page's nav selection.
  const [selectedId, setSelectedId] = useState(show.id)
  const [detail, setDetail] = useState<ProjectGroupShow | null>(show)
  const [detailError, setDetailError] = useState<string | null>(null)

  // Fetch the selected project's show view on selection change + every
  // project-group event (the store's revision — groups/members/poc/
  // layout changed all land here; the store already coalesces).
  useEffect(() => {
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

  // A selection whose group vanished (deleted on another client) falls
  // back to the page's project; if THAT is gone too, the page-level
  // store drops its own selection and this panel unmounts with it.
  useEffect(() => {
    if (groups !== null && !groups.some((g) => g.id === selectedId)) {
      setSelectedId(show.id)
    }
  }, [groups, selectedId, show.id])

  // Esc closes Settings (capture phase, so the page's Esc→Agents
  // listener never sees it). Editable targets keep their own local
  // Esc semantics (cancel rename / close picker).
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      const t = e.target as HTMLElement | null
      if (t && (t.closest('input, textarea, select') || t.isContentEditable)) return
      e.preventDefault()
      e.stopPropagation()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

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
    <div className="flex-1 flex flex-col min-h-0">
      {/* Panel header — title + close (the placeholder's anatomy). */}
      <div className="flex items-center gap-2 flex-shrink-0 px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-[11px] font-semibold text-[var(--color-text-primary)]">
          Project settings
        </span>
        <span className="flex-1" />
        <button
          type="button"
          onClick={onClose}
          className="flex items-center justify-center w-6 h-6 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] transition-colors cursor-pointer"
          title="Close settings (Esc)"
        >
          <svg width="10" height="10" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
            <line x1="2" y1="2" x2="10" y2="10" />
            <line x1="10" y1="2" x2="2" y2="10" />
          </svg>
        </button>
      </div>

      {/* Master-detail (§6.5 layout directive — the ProjectsSection
          column viewer: left selectable list, right selected details). */}
      <div className="flex-1 flex min-h-0">
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
          {detailError ? (
            <p className="text-[11px] text-red-400">Failed to load project: {detailError}</p>
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
                // Deleting the page's open project closes Settings with
                // it (the nav selection drops via groups-changed); a
                // sibling delete keeps the panel open on the fallback.
                if (selectedId === show.id) onClose()
                else setSelectedId(show.id)
              }}
            />
          )}
        </div>
      </div>
    </div>
  )
}
