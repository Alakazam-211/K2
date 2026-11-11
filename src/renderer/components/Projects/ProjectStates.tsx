// The Projects page's load, empty and failure states, one self-contained
// component each so they move with the layout (0.46.12).
//
// Page-sized states (the project list, one project, no dashboard, the chat,
// the Feedback tab) are the owner's state card (StateCard.tsx: an icon, a
// title, one plain line, the server's reason quietly, the next step). A tile
// is too small for a card in a card: `TileState` is the same anatomy,
// centred, with no card around it.

import React, { useEffect, useState } from 'react'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useWindowModeStore } from '@/stores/window-mode'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { primaryScope } from '@/kessel/server-scope'
import { CreateProjectForm } from './ProjectNav'
import { projectSettingsCaps, type ProjectRole } from './project-settings'
import { createErrorMessage, createProjectGroupDashboard } from './projects-api'
import {
  PlusGlyph,
  ProjectStateCard,
  ProjectsIcon,
  StateAction,
  StateCanvas as Canvas,
  StateRetry,
} from './StateCard'

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
        <ProjectStateCard
          kind="error"
          title="Couldn’t load your projects"
          detail={error}
          testId="projects-list-error"
          action={<StateRetry onClick={retry} />}
        >
          K2 couldn’t get the project list from the server. Your agents are fine.
        </ProjectStateCard>
      ) : (
        <ProjectStateCard kind="loading" title="Loading your projects…" testId="projects-list-loading" />
      )}
    </Canvas>
  )
}

/** No projects on this server yet: what a project is, and New project. */
export function ProjectsEmptyState(): React.JSX.Element {
  const [creating, setCreating] = useState(false)
  return (
    <Canvas>
      <ProjectStateCard
        kind="empty"
        icon={<ProjectsIcon />}
        title="No projects yet"
        testId="projects-empty"
        action={
          creating ? (
            <div className="w-full text-left">
              <CreateProjectForm onDone={() => setCreating(false)} />
            </div>
          ) : (
            <StateAction onClick={() => setCreating(true)} glyph={<PlusGlyph />}>
              New project
            </StateAction>
          )
        }
      >
        A project puts a few agents on one dashboard, with one shared chat.
      </ProjectStateCard>
    </Canvas>
  )
}

/** One project's members and dashboards are loading, or failed to load. */
export function ProjectLoadState({
  name,
  error,
  onRetry,
}: {
  /** The project's name, from the list (the card says which failed). */
  name: string | null
  error: string | null
  onRetry: () => void
}): React.JSX.Element {
  return (
    <Canvas>
      {error ? (
        <ProjectStateCard
          kind="error"
          title={name ? `Couldn’t load ${name}` : 'Couldn’t load this project'}
          detail={error}
          testId="project-load-error"
          action={<StateRetry onClick={onRetry} />}
        >
          K2 couldn’t get this project from the server. Your agents are fine.
        </ProjectStateCard>
      ) : (
        <ProjectStateCard kind="loading" title={name ? `Loading ${name}…` : 'Loading the project…'} testId="project-loading" />
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
        <ProjectStateCard
          kind="error"
          title="Couldn’t create the dashboard"
          detail={error}
          testId="project-dashboard-create-error"
          action={<StateRetry onClick={create} disabled={busy} />}
        >
          Nothing changed in this project.
        </ProjectStateCard>
      ) : (
        <ProjectStateCard
          kind="empty"
          icon={<ProjectsIcon />}
          title="This project has no dashboard yet"
          testId="project-no-dashboard"
          action={
            canCreate ? (
              <StateAction onClick={create} disabled={busy} glyph={<PlusGlyph />}>
                Create a dashboard
              </StateAction>
            ) : undefined
          }
        >
          {canCreate
            ? 'A dashboard is where the project’s agents sit side by side.'
            : 'Only the project’s owner or an admin can add one.'}
        </ProjectStateCard>
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
