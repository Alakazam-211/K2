// Projects V1 P4 (prd-projects-v1 §6.1) — the full-page Projects view.
//
// A top-level page selected from the §6.0 switcher; body idiom borrowed
// from FeedbackPage.tsx: a fixed-inset overlay with its own draggable
// top bar (which carries the ServerSwitcher + the 3-tab switcher, so the
// server dropdown stays visible here), Esc adapted to tab navigation
// (back to the Agents page). LEFT NAV = ProjectNav (Pinned + list +
// member drawer); the main area shows the selected project under
// Dashboard | Feedback tabs (the FeedbackItemView two-tab idiom) plus a
// gear opening the per-project Settings slot.
//
// P5 fills the Dashboard tab: ProjectDashboard renders the canonical
// layout blob as percent-width pane columns (kessel canonical-terminal
// attach + htmlDoc panes), with drag-and-drop, coalesced save-layout,
// and apply-on-open staleness (§6.2/§6.3). P7 fills the Feedback tab:
// ProjectFeedbackTab, the member-scoped feedback board (§6.6). P8
// fills the gear panel: ProjectSettings, the §6.5 master-detail
// per-project Settings surface (deep-linked here with the open
// project preselected).

import React, { useEffect, useMemo, useState } from 'react'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import ProjectNav, { CreateProjectForm } from './ProjectNav'
import ProjectDashboard from './ProjectDashboard'
import ProjectChatDrawer from './ProjectChatDrawer'
import { ProjectFeedbackTab } from '@/components/Feedback/ProjectFeedbackTab'
import ProjectSettings from './ProjectSettings'
import { fetchProjectGroupShow, type ProjectGroupShow } from './projects-api'

const TOPBAR_HEIGHT = 38

type ProjectTab = 'dashboard' | 'feedback'

// ── Selected-project main area ────────────────────────────────────────────

function DashboardTab({ show }: { show: ProjectGroupShow }): React.JSX.Element {
  // V1 renders the first (single 'Main') dashboard; the selector row
  // surfaces it — multi-dashboard UI is V1.1 (routes are id-addressed).
  const dashboard = show.dashboards[0] ?? null

  return (
    <div className="flex-1 flex flex-col min-h-0 p-3 gap-2">
      <div className="flex items-center gap-1 flex-shrink-0">
        {show.dashboards.map((d, idx) => (
          <span
            key={d.id}
            className={`px-2 py-1 text-[10px] font-medium border ${
              idx === 0
                ? 'border-[var(--color-accent)] bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'border-[var(--color-border)] text-[var(--color-text-secondary)]'
            }`}
            title={`Dashboard "${d.name}" — revision ${d.revision}`}
          >
            {d.name}
          </span>
        ))}
      </div>

      {/* P5 — the pane grid (§6.2/§6.3). Keyed by dashboard id so a
          project/dashboard switch is a fresh mount = apply-on-open;
          `show` refetches (same key) update props WITHOUT re-adopting
          the layout — the dashboard only marks itself stale. */}
      {dashboard ? (
        <ProjectDashboard key={dashboard.id} show={show} dashboard={dashboard} />
      ) : (
        <div className="flex-1 flex items-center justify-center border border-dashed border-[var(--color-border)] text-center px-8">
          <p className="text-xs text-[var(--color-text-muted)]">
            This project has no dashboard yet.
          </p>
        </div>
      )}

      {/* P6 (§6.4) — the project chat drawer lives on the Dashboard tab
          (NOT a separate tab). Chat is PER-PROJECT, so it renders even
          when the dashboard row is missing. Not keyed by dashboard —
          `show` refetches update props without remounting, so the
          composer draft survives them (SelectedProjectView is keyed by
          group id upstream: a project switch is a fresh drawer). */}
      <ProjectChatDrawer show={show} />
    </div>
  )
}

function SelectedProjectView({
  show,
  error,
}: {
  show: ProjectGroupShow | null
  error: string | null
}): React.JSX.Element {
  const [tab, setTab] = useState<ProjectTab>('dashboard')
  const [settingsOpen, setSettingsOpen] = useState(false)

  // Selection switches arrive as a fresh mount (the page keys this view
  // by group id), so tab/settings state resets per project.

  // P5 — a member-row click (open/focus its canonical pane) always
  // lands on the Dashboard tab; the mounted ProjectDashboard consumes
  // the request itself.
  const paneRequestNonce = useProjectGroupsStore((s) => s.paneRequest?.nonce ?? null)
  useEffect(() => {
    if (paneRequestNonce === null) return
    setTab('dashboard')
    setSettingsOpen(false)
  }, [paneRequestNonce])

  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center px-8">
        <p className="text-[11px] text-red-400">Failed to load project: {error}</p>
      </div>
    )
  }
  if (!show) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <p className="text-sm text-[var(--color-text-muted)]">Loading project…</p>
      </div>
    )
  }

  return (
    <div className="flex-1 flex flex-col min-h-0 min-w-0">
      {/* Project header: name + member count + tabs, gear right. */}
      <div className="px-4 pt-3 border-b border-[var(--color-border)] flex-shrink-0">
        <div className="flex items-center gap-3 min-w-0">
          <h1 className="text-sm font-medium text-[var(--color-text-primary)] truncate">
            {show.name}
          </h1>
          <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono flex-shrink-0">
            {show.memberCount} {show.memberCount === 1 ? 'member' : 'members'}
          </span>
          <span className="flex-1" />
          <button
            type="button"
            onClick={() => setSettingsOpen((v) => !v)}
            className={`flex h-6 w-6 items-center justify-center transition-colors cursor-pointer ${
              settingsOpen
                ? 'text-[var(--color-text-primary)] bg-white/[0.08]'
                : 'text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)]'
            }`}
            title="Project settings"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
            </svg>
          </button>
        </div>
        <div className="flex items-center gap-1 mt-2">
          {(['dashboard', 'feedback'] as const).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => {
                setTab(t)
                setSettingsOpen(false)
              }}
              className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
                tab === t && !settingsOpen
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
            >
              {t === 'dashboard' ? 'Dashboard' : 'Feedback'}
            </button>
          ))}
        </div>
      </div>

      {settingsOpen ? (
        /* P8 (§6.5) — the per-project Settings surface, deep-linked
           with THIS project preselected (its left list can switch). */
        <ProjectSettings show={show} onClose={() => setSettingsOpen(false)} />
      ) : tab === 'dashboard' ? (
        <DashboardTab show={show} />
      ) : (
        /* P7 (§6.6) — the member-scoped feedback board. Keyed upstream
           by group id (SelectedProjectView), so a project switch is a
           fresh list; `show` refetches only update the members prop. */
        <ProjectFeedbackTab members={show.members} />
      )}
    </div>
  )
}

// ── The page ──────────────────────────────────────────────────────────────

export default function ProjectsPage(): React.JSX.Element | null {
  const isOpen = usePageViewStore((s) => s.page === 'projects')
  const groups = useProjectGroupsStore((s) => s.groups)
  const selectedGroupId = useProjectGroupsStore((s) => s.selectedGroupId)
  const revision = useProjectGroupsStore((s) => s.revision)

  const [show, setShow] = useState<ProjectGroupShow | null>(null)
  const [showError, setShowError] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)

  // Fetch the group list while open: on open and on every project-group
  // event (revision) — the store also coalesce-refetches on events, so
  // this mainly covers open-with-stale-state.
  useEffect(() => {
    if (!isOpen) return
    void useProjectGroupsStore.getState().fetchGroups()
  }, [isOpen, revision])

  // Fetch the selected group's show view (members + dashboards) on
  // selection change and on events.
  useEffect(() => {
    if (!isOpen || !selectedGroupId) {
      setShow(null)
      setShowError(null)
      return
    }
    let cancelled = false
    fetchProjectGroupShow(selectedGroupId)
      .then((data) => {
        if (cancelled) return
        setShow(data)
        setShowError(null)
      })
      .catch((e) => {
        if (cancelled) return
        setShowError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [isOpen, selectedGroupId, revision])

  // Esc — back to the Agents page (FeedbackPage's Esc, adapted to tab
  // navigation; inputs stopPropagation their own Esc).
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        usePageViewStore.getState().setPage('agents')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen])

  // Reset transient view state when the page closes (selection is
  // store-level and deliberately survives — reopening lands where the
  // user left off).
  useEffect(() => {
    if (!isOpen) setCreating(false)
  }, [isOpen])

  const selectedGroup = useMemo(
    () => groups?.find((g) => g.id === selectedGroupId) ?? null,
    [groups, selectedGroupId],
  )

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-[var(--color-bg)]">
      {/* Top bar — mirrors the workspace TopBar's left cluster: traffic-
          light spacer + wordmark + SERVER DROPDOWN + the page switcher
          (§6.0: both visible on every page), draggable. */}
      <div
        className="flex items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none flex-shrink-0"
        data-tauri-drag-region
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div className="flex items-center gap-2 flex-1">
          <div style={{ width: 70 }} />
          <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">
            K2
          </span>
          <ServerSwitcher />
          <div className="no-drag">
            <PageTabs />
          </div>
        </div>
      </div>

      {/* Body: left nav + main area. */}
      <div className="flex-1 min-h-0 flex">
        <ProjectNav
          members={show?.id === selectedGroupId ? show?.members ?? null : null}
          pocWorkspaceId={show?.pocWorkspaceId ?? null}
        />

        <div className="flex-1 flex flex-col min-h-0 min-w-0">
          {groups === null ? (
            <div className="flex-1 flex items-center justify-center">
              <p className="text-sm text-[var(--color-text-muted)]">Loading projects…</p>
            </div>
          ) : groups.length === 0 ? (
            /* Empty state — no projects exist yet (create CTA). */
            <div className="flex-1 flex items-center justify-center px-8">
              <div className="w-full max-w-sm text-center">
                <p className="text-sm text-[var(--color-text-secondary)]">No projects yet</p>
                <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
                  A project groups agents into one effort — a shared dashboard, one chat, one
                  Point of Contact.
                </p>
                <div className="mt-4 text-left">
                  {creating ? (
                    <CreateProjectForm onDone={() => setCreating(false)} />
                  ) : (
                    <button
                      className="w-full flex items-center justify-center gap-2 px-3 py-2 text-xs bg-white/[0.04] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.08] transition-colors"
                      onClick={() => setCreating(true)}
                    >
                      <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
                      </svg>
                      Create your first project
                    </button>
                  )}
                </div>
              </div>
            </div>
          ) : !selectedGroup ? (
            <div className="flex-1 flex items-center justify-center m-4 border border-dashed border-[var(--color-border)] text-xs text-[var(--color-text-muted)] text-center px-6">
              Select a project to open its dashboard.
            </div>
          ) : (
            /* Same staleness guard as the nav's member drawer: while a
               selection switch's `show` fetch is in flight, render the
               loading state — never the PREVIOUS project's data (P5:
               that would briefly mount the wrong project's panes). */
            <SelectedProjectView
              key={selectedGroup.id}
              show={show?.id === selectedGroup.id ? show : null}
              error={showError}
            />
          )}
        </div>
      </div>
    </div>
  )
}
