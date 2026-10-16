// The Agent drawer tab (0.46.0, Rosson 2026-10-16): the room's agent, live.
//
//   1. Profile image   2. Color   3. Agent name + handle   4. Permissions
//
// Nothing here is a second form. Every control is the one Settings →
// Workspaces → Agent uses (`components/AgentIdentity` for the identity,
// the exported permission rows in `ProjectsSection` with `compact`), so
// both write through the same routes and store actions, and a change in
// either shows up everywhere at once (the daemon emits
// `agent_identity_changed` / SyncProjects; every client patches from it).
//
// Where it can edit:
// - The window's own room (`room.isPrimary`): editable. Those controls
//   write to the window's server and paint the window's projects store.
// - A room on another server (Home): read-only. Identity and permission
//   writes are server management, not room work, so they are not on the
//   room write allowlist (`kessel/room-writes.ts`); "Open <server>" does
//   them. The tab still shows that agent's picture, color, name, handle and
//   switches, and follows renames live on the room's own server.
//
// Roles (Connect logins on the window's server): the switches follow the
// route floors the daemon enforces (`route_policy.rs`). A switch whose
// route is above the viewer's role shows its state but can't be flipped.
// Name, handle, picture and color are Member-floor routes.

import { useCallback, useEffect, useState } from 'react'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { onAgentIdentityChanged } from '@/stores/session-events'
import { agentIdentityDisplayName, agentIdentityMatchesPath } from '@/lib/agent-identity'
import { agentDisplayName, agentHandle } from '@/lib/workspace-agent'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import {
  AgentColorPicker,
  AgentDisplayNameField,
  AgentHandleField,
  AgentPictureControls,
  AgentPictureTile,
} from '@/components/AgentIdentity/AgentIdentityFields'
import {
  AgentsCreateConnectionsToggle,
  AgentsManageAgentsToggle,
  AgentsManageProjectsToggle,
  AgentsManageSkinToggle,
  AgentsManageVaultToggle,
  AgentsOperateComputeToggle,
  AgentsUseComputeToggle,
  DbAgentCreateToggle,
  DnsManageToggle,
  MailManageToggle,
  RemoteInstructToggle,
} from '@/components/Settings/sections/ProjectsSection'

type ViewerRole = 'owner' | 'admin' | 'member'
type RouteFloor = ViewerRole

const RANK: Record<ViewerRole, number> = { member: 0, admin: 1, owner: 2 }

/** May `role` flip a switch whose route needs `floor`? Unknown role (an
 *  older daemon, whoami not answered yet) leaves it to the daemon. */
export function roleMeetsFloor(role: ViewerRole | null, floor: RouteFloor): boolean {
  if (role === null) return true
  return RANK[role] >= RANK[floor]
}

function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] mb-2">
      {children}
    </div>
  )
}

/** Name + handle for a room on another server: text, live from that
 *  server's `agent_identity_changed`. */
function ReadOnlyNameAndHandle({ project }: { project: ProjectWithWorkspaces }): React.JSX.Element {
  const room = useRoom()
  const [name, setName] = useState('')
  const [handle, setHandle] = useState('')

  useEffect(() => {
    let cancelled = false
    void agentDisplayName(room.scope, project.path)
      .then((n) => { if (!cancelled) setName(n) })
      .catch(() => { if (!cancelled) setName('') })
    void agentHandle(room.scope, project.path)
      .then((h) => { if (!cancelled) setHandle(h) })
      .catch(() => { if (!cancelled) setHandle('') })
    const off = onAgentIdentityChanged(room.scope, (e) => {
      if (cancelled || !agentIdentityMatchesPath(e, project.path)) return
      const n = agentIdentityDisplayName(e)
      if (n) setName(n)
      if (e.handle) setHandle(e.handle)
    })
    return () => { cancelled = true; off() }
  }, [room, project.path])

  return (
    <div className="space-y-2">
      <div>
        <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Agent Name</div>
        <div className="text-xs text-[var(--color-text-primary)] truncate" data-testid="agent-name-readonly">
          {name || project.name}
        </div>
      </div>
      <div>
        <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Handle</div>
        <div className="text-xs font-mono text-[var(--color-text-primary)] truncate" data-testid="agent-handle-readonly">
          {handle || project.handle || ''}
        </div>
      </div>
    </div>
  )
}

export default function AgentPanel(): React.JSX.Element {
  const room = useRoom()
  const project = useRoomProjects(useCallback((projects: ProjectWithWorkspaces[]) => {
    const id = room.activeProjectId()
    return id ? projects.find((p) => p.id === id) ?? null : null
  }, [room]))
  const editable = room.isPrimary

  // The viewer's role on the window's server (owner token → owner). Only
  // the window's own room edits, so only it asks.
  const [role, setRole] = useState<ViewerRole | null>(null)
  useEffect(() => {
    if (!editable) return
    let cancelled = false
    void fetchViewerRole().then((r) => { if (!cancelled) setRole(r) })
    return () => { cancelled = true }
  }, [editable])

  // Event-handler refetch for the three switches that still re-read the
  // list after a write (as they do in Settings); never called on render.
  const refetchProjects = useCallback(() => useProjectsStore.getState().fetchProjects(), [])

  if (!project) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">No agent selected</p>
      </div>
    )
  }

  const ro = (floor: RouteFloor): boolean => !editable || !roleMeetsFloor(role, floor)

  return (
    <div className="h-full min-h-0 overflow-y-auto" data-testid="agent-panel">
      {!editable && (
        <div
          className="px-3 py-2 border-b border-[var(--color-border)] text-[10px] text-[var(--color-text-muted)] leading-relaxed"
          data-testid="agent-panel-readonly-note"
        >
          This agent is on {room.scope.label}. Open that server to change it.
        </div>
      )}

      {/* ── 1. Profile image ── */}
      <section className="px-3 py-3 border-b border-[var(--color-border)]" aria-label="Profile image">
        <SectionTitle>Profile image</SectionTitle>
        {editable
          ? <AgentPictureControls scope={room.scope} project={project} compact />
          : <AgentPictureTile project={project} size={40} />}
      </section>

      {/* ── 2. Color ── */}
      <section className="px-3 py-3 border-b border-[var(--color-border)]" aria-label="Color">
        <SectionTitle>Color</SectionTitle>
        {editable ? (
          <AgentColorPicker project={project} compact />
        ) : (
          <div className="flex items-center gap-2">
            <span className="w-4 h-4 flex-shrink-0" style={{ backgroundColor: project.color }} />
            <span className="text-[11px] font-mono text-[var(--color-text-secondary)]">{project.color}</span>
          </div>
        )}
      </section>

      {/* ── 3. Agent name and handle ── */}
      <section className="px-3 py-3 border-b border-[var(--color-border)]" aria-label="Agent name and handle">
        {editable ? (
          <>
            <AgentDisplayNameField scope={room.scope} projectPath={project.path} />
            <AgentHandleField
              scope={room.scope}
              projectPath={project.path}
              projectHandle={project.handle}
              compact
            />
          </>
        ) : (
          <ReadOnlyNameAndHandle project={project} />
        )}
      </section>

      {/* ── 4. Permissions ── the Settings rows, compact. Order matches
          Settings → Workspaces → Agent, then Connections. */}
      <section className="py-3" aria-label="Permissions">
        <div className="px-3"><SectionTitle>Permissions</SectionTitle></div>
        <div className="divide-y divide-[var(--color-border)]">
          <RemoteInstructToggle project={project} fetchProjects={refetchProjects} compact readOnly={ro('admin')} />
          <DnsManageToggle project={project} fetchProjects={refetchProjects} compact readOnly={ro('admin')} />
          <MailManageToggle project={project} compact readOnly={ro('admin')} />
          <DbAgentCreateToggle project={project} compact readOnly={ro('member')} />
          <AgentsManageSkinToggle project={project} compact readOnly={ro('owner')} />
          <AgentsManageAgentsToggle project={project} compact readOnly={ro('admin')} />
          <AgentsManageVaultToggle project={project} compact readOnly={ro('admin')} />
          <AgentsUseComputeToggle project={project} compact readOnly={ro('admin')} />
          <AgentsOperateComputeToggle project={project} compact readOnly={!editable} />
          <AgentsManageProjectsToggle project={project} compact readOnly={ro('admin')} />
          <AgentsCreateConnectionsToggle project={project} fetchProjects={refetchProjects} compact readOnly={ro('admin')} />
        </div>
        {editable && (
          <div className="px-3 pt-2">
            <button
              type="button"
              className="text-[10px] text-[var(--color-accent)] hover:underline bg-transparent border-none p-0 cursor-pointer no-drag"
              onClick={() => useSettingsStore.getState().openSettings('projects', project.id, 'agent')}
            >
              More in Settings
            </button>
          </div>
        )}
      </section>
    </div>
  )
}
