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
// P4 ships the SHELL: the Dashboard tab renders the project's dashboards
// from `show` ('Main' only in V1) with a placeholder pane region P5
// fills (DnD columns + kessel attach); the Feedback tab is P7's slot;
// the gear panel is P8's slot. Each seam is marked with a TODO.

import React, { useEffect, useMemo, useState } from 'react'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import ServerSwitcher from '@/components/TopBar/ServerSwitcher'
import PageTabs from '@/components/TopBar/PageTabs'
import ProjectNav, { CreateProjectForm } from './ProjectNav'
import { fetchProjectGroupShow, type ProjectGroupShow } from './projects-api'

const TOPBAR_HEIGHT = 38

type ProjectTab = 'dashboard' | 'feedback'

// ── Selected-project main area ────────────────────────────────────────────

function DashboardTab({ show }: { show: ProjectGroupShow }): React.JSX.Element {
  const poc = show.members.find((m) => m.workspaceId === show.pocWorkspaceId)
  const pocName = poc ? (poc.agentName ?? poc.name ?? 'the PoC') : null

  return (
    <div className="flex-1 flex flex-col min-h-0 p-3 gap-2">
      {/* Dashboard selector row — V1 surfaces the single 'Main' row;
          multi-dashboard UI is V1.1 (schema/routes are id-addressed). */}
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

      {/* TODO(P5): the pane region — drag-and-drop percent-width columns
          of panes (agent canonical terminals via the attachAgentName
          idiom + pinned-HTML FileViewerPane reuse), parsed from
          dashboards[0].layoutJson (§6.2/§6.3), save-layout on change,
          apply-on-open, stale-on-layout-changed. This placeholder is the
          slot it replaces. */}
      <div
        data-projects-dashboard-panes
        className="flex-1 flex items-center justify-center border border-dashed border-[var(--color-border)] text-center px-8"
      >
        <div>
          <p className="text-sm text-[var(--color-text-secondary)]">No panes yet</p>
          <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
            {pocName
              ? `A fresh dashboard starts with ${pocName}'s canonical terminal — pane layout arrives in the next slice.`
              : 'Add a member to this project — its agent becomes the Point of Contact and the first pane.'}
          </p>
        </div>
      </div>
    </div>
  )
}

function FeedbackTabPlaceholder({ show }: { show: ProjectGroupShow }): React.JSX.Element {
  // TODO(P7): the project-scoped feedback list — fetchAllFeedback
  // restricted to this project's member workspaces + the recycled
  // FeedbackItemView master-detail (§6.6). This placeholder is its slot.
  return (
    <div className="flex-1 flex items-center justify-center m-3 border border-dashed border-[var(--color-border)] text-center px-8">
      <div>
        <p className="text-sm text-[var(--color-text-secondary)]">Project feedback</p>
        <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
          Open asks from {show.name}&rsquo;s {show.members.length}{' '}
          {show.members.length === 1 ? 'member' : 'members'} will show here.
        </p>
      </div>
    </div>
  )
}

function SettingsPlaceholder({
  show,
  onClose,
}: {
  show: ProjectGroupShow
  onClose: () => void
}): React.JSX.Element {
  // TODO(P8): the per-project Settings page — master-detail copied from
  // the Workspaces settings column viewer (§6.5): dashboards (Main row),
  // members add/remove w/ the PoC-successor rule, PoC dropdown, the
  // html-docs browser, delete project. The gear deep-links here with
  // this project preselected. This panel is its slot.
  return (
    <div className="flex-1 flex flex-col min-h-0 p-3">
      <div className="flex items-center gap-2 flex-shrink-0 pb-2">
        <span className="text-[11px] font-semibold text-[var(--color-text-primary)]">
          {show.name} — Settings
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
      <div className="flex-1 flex items-center justify-center border border-dashed border-[var(--color-border)] text-center px-8">
        <div>
          <p className="text-sm text-[var(--color-text-secondary)]">Project settings</p>
          <p className="text-xs text-[var(--color-text-muted)] mt-1 opacity-70">
            Dashboards, members, Point of Contact, and pinned HTML docs are managed here soon.
          </p>
        </div>
      </div>
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
        <SettingsPlaceholder show={show} onClose={() => setSettingsOpen(false)} />
      ) : tab === 'dashboard' ? (
        <DashboardTab show={show} />
      ) : (
        <FeedbackTabPlaceholder show={show} />
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
            <SelectedProjectView key={selectedGroup.id} show={show} error={showError} />
          )}
        </div>
      </div>
    </div>
  )
}
