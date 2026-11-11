// The Projects page's load, empty and failure states, one self-contained
// component each so they move with the layout (0.46.12).
//
// Page-sized states (the project list, one project, no dashboard, the chat,
// the Feedback tab) are the shared state card (`SettingsLoadState`: title,
// one plain line, the server's reason quietly, the next step). A tile is too
// small for a card in a card: `TileState` is the same anatomy, centred, with
// no card around it.

import React, { useEffect, useState } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useWindowModeStore } from '@/stores/window-mode'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { SettingsLoadState } from '@/components/Settings/controls/SettingControls'
import { Button } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { CreateProjectForm } from './ProjectNav'
import { projectSettingsCaps, type ProjectRole } from './project-settings'
import { createErrorMessage, createProjectGroupDashboard } from './projects-api'
import { TryAgain } from './TileState'

/** A page-sized state, centred on the canvas. */
function Canvas({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex-1 min-h-0 flex items-center justify-center p-6">
      <div className="w-full max-w-md">{children}</div>
    </div>
  )
}

/** The project list is loading, or its read failed (never an endless
 *  "Loading…": the store keeps the failure in `groupsError`). */
export function ProjectListLoadState(): React.JSX.Element {
  const error = useProjectGroupsStore((s) => s.groupsError)
  const [retrying, setRetrying] = useState(false)
  const retry = (): void => {
    setRetrying(true)
    void useProjectGroupsStore
      .getState()
      .fetchGroups()
      .finally(() => setRetrying(false))
  }
  return (
    <Canvas>
      {error && !retrying ? (
        <SettingsLoadState
          kind="error"
          title="Couldn’t load your projects"
          detail={error}
          testId="projects-list-error"
          action={<TryAgain onClick={retry} />}
        >
          K2 couldn’t read the project list from this server.
        </SettingsLoadState>
      ) : (
        <SettingsLoadState kind="loading" title="Loading your projects…" testId="projects-list-loading" />
      )}
    </Canvas>
  )
}

/** No projects on this server yet: what a project is, and New project. */
export function ProjectsEmptyState(): React.JSX.Element {
  const [creating, setCreating] = useState(false)
  return (
    <Canvas>
      <SettingsLoadState
        kind="empty"
        stack
        title="No projects yet"
        testId="projects-empty"
        action={
          creating ? (
            <div className="w-full text-left">
              <CreateProjectForm onDone={() => setCreating(false)} />
            </div>
          ) : (
            <Button variant="accent" onClick={() => setCreating(true)}>
              New project
            </Button>
          )
        }
      >
        A project keeps the agents working on one goal together, on a shared dashboard with one chat.
      </SettingsLoadState>
    </Canvas>
  )
}

/** One project's members and dashboards are loading, or failed to load. */
export function ProjectLoadState({
  error,
  onRetry,
}: {
  error: string | null
  onRetry: () => void
}): React.JSX.Element {
  return (
    <Canvas>
      {error ? (
        <SettingsLoadState
          kind="error"
          title="Couldn’t open this project"
          detail={error}
          testId="project-load-error"
          action={<TryAgain onClick={onRetry} />}
        >
          K2 couldn’t read its agents and dashboards from this server.
        </SettingsLoadState>
      ) : (
        <SettingsLoadState kind="loading" title="Loading the project…" testId="project-loading" />
      )}
    </Canvas>
  )
}

/** The signed-in person's role on this server (null until whoami answers,
 *  or when it can't say: the daemon decides then). */
function useViewerRole(): ProjectRole | null {
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

/** A project with no dashboard: an owner or admin can make one here;
 *  anyone else is told who can. */
export function NoDashboardState({
  groupId,
  onCreated,
}: {
  groupId: string
  onCreated: () => void
}): React.JSX.Element {
  const role = useViewerRole()
  const viewer = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const canCreate = !viewer && projectSettingsCaps(role).canEditDashboards
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const create = (): void => {
    setBusy(true)
    setError(null)
    createProjectGroupDashboard(primaryScope(), groupId, 'Main')
      .then(() => onCreated())
      .catch((e) => setError(createErrorMessage(e)))
      .finally(() => setBusy(false))
  }
  return (
    <Canvas>
      {error ? (
        <SettingsLoadState
          kind="error"
          title="Couldn’t create the dashboard"
          detail={error}
          testId="project-dashboard-create-error"
          action={<TryAgain onClick={create} busy={busy} />}
        >
          Nothing changed in this project.
        </SettingsLoadState>
      ) : (
        <SettingsLoadState
          kind="empty"
          title="This project has no dashboard yet"
          testId="project-no-dashboard"
          action={
            canCreate ? (
              <Button variant="accent" onClick={create} disabled={busy}>
                Create a dashboard
              </Button>
            ) : undefined
          }
        >
          {canCreate
            ? 'A dashboard is where the project’s agents sit side by side.'
            : 'Only the project’s owner or an admin can add one.'}
        </SettingsLoadState>
      )}
    </Canvas>
  )
}

/** What an empty dashboard tells the person to do next. */
export function emptyDashboardHint(memberCount: number, readOnly: boolean): string {
  if (readOnly) return 'Agents show here once the project’s owner or an admin adds them.'
  if (memberCount === 0) return 'Add an agent to this project in its settings, then drag it here.'
  return 'Drag an agent from Members onto this space, or click one to open it.'
}
