import { useEffect, useMemo, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import {
  useHeartbeatSessionsStore,
  subscribeHeartbeatLive,
  unsubscribeHeartbeatLive,
  type HeartbeatEntry,
} from '@/stores/heartbeat-sessions'
import { serverSupports } from '@/lib/server-capabilities'
import { IconHeartEKG } from '@/components/icons/IconHeartEKG'
import { HeartbeatEntryRow } from './HeartbeatEntry'

/**
 * Heartbeats section — workspace-scoped audit surface for scheduled
 * heartbeat chat sessions. Mounted inside `WorkspacePanel` directly
 * beneath the State row, since workspace state limits/enables which
 * heartbeats are allowed to fire (they share fate).
 *
 * Sections:
 *   - Live      : PTY currently running (braille spinner indicator)
 *   - Resumable : has a saved session_id, no live PTY (filled dot)
 *   - Scheduled : configured but never fired (hollow dot)
 *   - Archived  : collapsed by default, persisted per-workspace
 *
 * Header carries the heart-with-EKG icon and a `manage` link that
 * opens Settings → Heartbeats for full CRUD. Empty / off-mode state
 * shows guidance text rather than disappearing entirely so the
 * section never looks broken.
 */
export function HeartbeatsPanel(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const project = useMemo(
    () => projects.find((p) => p.id === activeProjectId) ?? null,
    [projects, activeProjectId],
  )
  const projectPath = project?.path ?? null
  const agentMode = project?.agentMode ?? 'off'

  const active = useHeartbeatSessionsStore((s) => s.active)
  const archived = useHeartbeatSessionsStore((s) => s.archived)
  const loadedFor = useHeartbeatSessionsStore((s) => s.loadedFor)
  const lastError = useHeartbeatSessionsStore((s) => s.lastError)
  const refresh = useHeartbeatSessionsStore((s) => s.refresh)
  const clear = useHeartbeatSessionsStore((s) => s.clear)

  // Initial snapshot on workspace switch. Liveness then comes from the
  // daemon's `heartbeat_state_changed` broadcast (push-primary, #677.1);
  // against an older/remote daemon that doesn't emit it we keep the 5s
  // poll fallback so the live-dot still converges.
  useEffect(() => {
    if (!projectPath || agentMode === 'off') {
      clear()
      unsubscribeHeartbeatLive()
      return
    }
    refresh(projectPath)
    if (serverSupports('daemon-broadcasts')) {
      subscribeHeartbeatLive(projectPath)
      return () => unsubscribeHeartbeatLive()
    }
    const t = setInterval(() => refresh(projectPath), 5000)
    return () => clearInterval(t)
  }, [projectPath, agentMode, refresh, clear])

  // Per-workspace localStorage key for the Archived section's collapse
  // state. Strictly tied to project.id (not path or undefined) so the
  // user's "I always collapse archived for project X" preference
  // survives workspace switches.
  const archivedKey = project ? `heartbeats.archive-collapsed.${project.id}` : null
  const [archivedOpen, setArchivedOpen] = useState<boolean>(() => {
    if (!archivedKey) return false
    return localStorage.getItem(archivedKey) === 'open'
  })
  // Reset to the new workspace's persisted value on switch.
  useEffect(() => {
    if (!archivedKey) {
      setArchivedOpen(false)
      return
    }
    setArchivedOpen(localStorage.getItem(archivedKey) === 'open')
  }, [archivedKey])

  // Whole-section collapse state, persisted per-workspace. Default
  // OPEN — most users expect to see their heartbeats at a glance.
  const sectionKey = project ? `heartbeats.section-collapsed.${project.id}` : null
  const [sectionOpen, setSectionOpen] = useState<boolean>(() => {
    if (!sectionKey) return true
    return localStorage.getItem(sectionKey) !== 'closed'
  })
  useEffect(() => {
    if (!sectionKey) {
      setSectionOpen(true)
      return
    }
    setSectionOpen(localStorage.getItem(sectionKey) !== 'closed')
  }, [sectionKey])

  // The Workspace panel only mounts this section when a project is
  // selected and agentMode !== 'off', so we no longer need the
  // standalone empty-state messages. Defensive guard kept (returns
  // nothing) so imports elsewhere don't crash on edge cases.
  if (!project || agentMode === 'off') {
    return <></>
  }

  // Within each section (Resumable / Scheduled / Live), pin disabled
  // rows to the bottom — they don't fire on their own, so they're
  // visually de-prioritized. Stable sort preserves the store's
  // alphabetical order within each enabled / disabled group, and the
  // section grouping still reflects the heartbeat's actual state
  // (resumable stays resumable, just sorted last).
  const sortedActive = [...active].sort(
    (a, b) => Number(b.row.enabled) - Number(a.row.enabled),
  )
  const live = sortedActive.filter((e) => e.state === 'live')
  const resumable = sortedActive.filter((e) => e.state === 'resumable')
  const scheduled = sortedActive.filter((e) => e.state === 'scheduled')
  const showingForLoadedProject = loadedFor === projectPath

  const toggleArchived = (): void => {
    const next = !archivedOpen
    setArchivedOpen(next)
    if (archivedKey) localStorage.setItem(archivedKey, next ? 'open' : 'closed')
  }

  const toggleSection = (): void => {
    const next = !sectionOpen
    setSectionOpen(next)
    if (sectionKey) localStorage.setItem(sectionKey, next ? 'open' : 'closed')
  }

  return (
    <div>
      {/* Header — full-width clickable collapse/expand bar. Same
          structural pattern as the Worktrees button below in
          WorkspacePanel: px-3 py-2, edge-to-edge bottom border so
          the divider runs the full width of the workspace panel. */}
      <button
        onClick={toggleSection}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={sectionOpen ? 'Collapse Heartbeats' : 'Expand Heartbeats'}
      >
        <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
          <svg
            className={`w-2 h-2 text-[var(--color-text-muted)] transition-transform ${sectionOpen ? 'rotate-90' : ''}`}
            viewBox="0 0 8 8"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M2 1 L6 4 L2 7" />
          </svg>
          <IconHeartEKG className="w-3 h-3 text-[var(--color-accent)]" />
          Heartbeats
          {/* Count badge — active (non-archived) only, matching the
              Worktrees count next to the Worktrees header below.
              Archived rows already live in their own collapsed
              section inside the panel; counting them here would
              inflate the visible "you have N heartbeats" cue. */}
          {active.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-white/5 text-[var(--color-text-muted)]">
              {active.length}
            </span>
          )}
        </span>
        <span
          onClick={(e) => {
            e.stopPropagation()
            // Pass the active project's id so Settings opens to THIS
            // workspace's heartbeats, not whichever workspace the
            // Settings module last had selected. Same call shape the
            // Sidebar's right-click → Workspace Settings uses.
            useSettingsStore.getState().openSettings('projects', project.id)
          }}
          className="text-[9px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
          title="Manage in Settings"
        >
          manage
        </span>
      </button>

      {sectionOpen && (
        <div className="px-3 py-2 border-b border-[var(--color-border)]">
      {!showingForLoadedProject ? (
        <div className="px-1 py-1 text-[10px] text-[var(--color-text-muted)]">Loading…</div>
      ) : lastError ? (
        // Surface the error rather than silently showing an empty state —
        // a broken Tauri command would otherwise render as "no heartbeats yet".
        <div className="px-1 py-1 text-[10px] text-red-400 leading-relaxed">
          Failed to load: {lastError}
        </div>
      ) : active.length === 0 && archived.length === 0 ? (
        <div className="px-1 py-1 text-[10px] text-[var(--color-text-muted)] leading-relaxed">
          No heartbeats yet.
        </div>
      ) : (
        <div className="space-y-1">
          {/* Active sections — render in display priority order so live work
              sits at the top of the panel where it's most visible. */}
          {live.length > 0 && (
            <Section title="Live" entries={live} projectPath={projectPath ?? ''} />
          )}
          {resumable.length > 0 && (
            <Section title="Resumable" entries={resumable} projectPath={projectPath ?? ''} />
          )}
          {scheduled.length > 0 && (
            <Section title="Scheduled" entries={scheduled} projectPath={projectPath ?? ''} />
          )}
          {/* Archived section — collapsed by default, stays on disk for
              auditability of retired heartbeats. */}
          {archived.length > 0 && (
            <div>
              <button
                onClick={toggleArchived}
                className="w-full flex items-center gap-1.5 px-1 py-1 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] cursor-pointer no-drag"
                title={archivedOpen ? 'Collapse Archived' : 'Expand Archived'}
              >
                <svg
                  className={`w-2 h-2 transition-transform ${archivedOpen ? 'rotate-90' : ''}`}
                  viewBox="0 0 8 8"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M2 1 L6 4 L2 7" />
                </svg>
                <span>Archived ({archived.length})</span>
              </button>
              {archivedOpen && (
                <div className="space-y-0.5">
                  {archived.map((entry) => (
                    <HeartbeatEntryRow key={entry.row.id} entry={entry} projectPath={projectPath ?? ''} />
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      )}
        </div>
      )}
    </div>
  )
}

function Section({
  title,
  entries,
  projectPath,
}: {
  title: string
  entries: HeartbeatEntry[]
  projectPath: string
}): React.JSX.Element {
  return (
    <div>
      <div className="px-1 py-0.5 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
        {title}
      </div>
      <div className="space-y-0.5">
        {entries.map((entry) => (
          <HeartbeatEntryRow key={entry.row.id} entry={entry} projectPath={projectPath} />
        ))}
      </div>
    </div>
  )
}
