import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { activateProject } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { SectionManageCog } from './SectionManageCog'
import { scopedKey } from '@/kessel/server-scope'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import { parseConnectionRows, remoteConnRowView, type ConnListRow } from './connected-agents-rows'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { AgentsIcon } from '@/components/icons/K2Icons'
import { useHomeRowAvatar, useHomeRowColor } from '@/lib/home-avatars'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { DRAWER_HEADER_GLYPH_PX, DRAWER_ROW_AVATAR_PX } from './drawer-glyph'

/** Collapsible Connected Agents — only workspaces with a present connection. */
export function ConnectedAgentsSection({ projectId }: { projectId: string }): React.JSX.Element {
  // Home M4: the room's server and its own project list (MS3).
  const room = useRoom()
  const collapseKey = scopedKey(room.scope, `connected-agents.section-collapsed.${projectId}`)
  const [open, setOpen] = useState<boolean>(() => localStorage.getItem(collapseKey) !== 'closed')
  const [rows, setRows] = useState<ConnListRow[]>([])
  const [loaded, setLoaded] = useState(false)
  const projects = useRoomProjects((p) => p)
  const project = projects.find((p) => p.id === projectId) ?? null
  const projectPath = project?.path ?? ''

  useEffect(() => {
    setOpen(localStorage.getItem(collapseKey) !== 'closed')
  }, [collapseKey])

  const toggle = useCallback((): void => {
    setOpen((cur) => {
      const next = !cur
      localStorage.setItem(collapseKey, next ? 'open' : 'closed')
      return next
    })
  }, [collapseKey])

  useEffect(() => {
    let cancelled = false
    setLoaded(false)
    if (!projectPath) {
      setRows([])
      setLoaded(true)
      return
    }
    const load = async (): Promise<void> => {
      try {
        const body = await daemonCliGet<unknown>(room.scope, 'connections', {
          project: projectPath,
          action: 'list',
        })
        if (cancelled) return
        setRows(parseConnectionRows(body))
      } catch {
        if (!cancelled) setRows([])
      } finally {
        if (!cancelled) setLoaded(true)
      }
    }
    void load()
    return () => {
      cancelled = true
    }
  }, [projectPath])

  const count = rows.length

  return (
    <>
      <button
        onClick={toggle}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={open ? 'Collapse Connected Agents' : 'Expand Connected Agents'}
      >
        <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
          <svg
            className={`w-2 h-2 text-[var(--color-text-muted)] transition-transform ${open ? 'rotate-90' : ''}`}
            viewBox="0 0 8 8"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M2 1 L6 4 L2 7" />
          </svg>
          <AgentsIcon size={DRAWER_HEADER_GLYPH_PX} className="text-[var(--color-accent)] flex-shrink-0" />
          Connected Agents
          {count > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {count}
            </span>
          )}
        </span>
        {/* Workspace settings are the window's server's (plan decision 6). */}
        {room.isPrimary && <SectionManageCog
          title="Manage connections"
          onClick={() => {
            useSettingsStore.getState().openSettings('projects', projectId, 'connections')
          }}
        />}
      </button>

      {open && (
        <div className="px-3 py-2 border-b border-[var(--color-border)] space-y-1.5">
          {!loaded ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">Loading…</p>
          ) : count === 0 ? (
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              No connected agents yet.
            </p>
          ) : (
            rows.map((row, i) => {
              if (row.remote) {
                const view = remoteConnRowView(row)
                return (
                  <ConnRow
                    key={row.address || `remote-${i}`}
                    title={view.title}
                    detail={view.detail}
                    avatar={<RemoteConnAvatar address={row.address ?? ''} name={view.title} gone={view.gone} />}
                    gone={view.gone}
                    // Re-add lives in Settings → Connections (owner/admin).
                    onClick={
                      view.gone && room.isPrimary
                        ? () => useSettingsStore.getState().openSettings('projects', projectId, 'connections')
                        : undefined
                    }
                  />
                )
              }
              const local = row.projectId
                ? projects.find((p) => p.id === row.projectId)
                : undefined
              const title = row.projectName || local?.name || 'Unknown workspace'
              return (
                <ConnRow
                  key={row.projectId || `local-${i}`}
                  title={title}
                  detail={row.reachable === false ? 'unreachable' : 'connected'}
                  avatar={
                    <LocalConnAvatar
                      project={local}
                      projectId={row.projectId}
                      name={title}
                      fetchIcon={room.isPrimary}
                    />
                  }
                  onClick={
                    row.projectId ? () => activateProject(row.projectId as string) : undefined
                  }
                />
              )
            })
          )}
        </div>
      )}
    </>
  )
}

/** A workspace on this room's server: its own avatar (picture, initial,
 *  colour), the shape the look gives every avatar. In a room on another
 *  server the path means nothing to the window's daemon, so only the
 *  listing's picture paints (no icon lookup). */
function LocalConnAvatar({
  project,
  projectId,
  name,
  fetchIcon,
}: {
  project: ProjectWithWorkspaces | undefined
  projectId: string | undefined
  name: string
  fetchIcon: boolean
}): React.JSX.Element {
  return (
    <span className="flex-shrink-0 flex" data-conn-avatar="local">
      <ProjectAvatar
        projectPath={project?.path ?? `conn:${projectId ?? name}`}
        projectName={project?.name ?? name}
        projectColor={project?.color || 'var(--color-neutral)'}
        projectId={project?.id}
        iconUrl={project?.iconUrl ?? null}
        size={DRAWER_ROW_AVATAR_PX}
        fetchIcon={fetchIcon && !!project}
      />
    </span>
  )
}

/** An agent on another server: the picture and colour the Home nav keeps
 *  for its address (`agent::host`), else its initial on the accent. A gone
 *  one (0159) is neutral and dimmed. */
function RemoteConnAvatar({ address, name, gone }: { address: string; name: string; gone: boolean }): React.JSX.Element {
  const picture = useHomeRowAvatar(address)
  const color = useHomeRowColor(address)
  return (
    <span className={`flex-shrink-0 flex ${gone ? 'opacity-60 grayscale' : ''}`} data-conn-avatar="remote">
      <ProjectAvatar
        projectPath={`home:${address}`}
        projectName={name}
        projectColor={gone ? 'var(--color-neutral)' : color ?? 'var(--color-accent)'}
        iconUrl={picture}
        size={DRAWER_ROW_AVATAR_PX}
        fetchIcon={false}
      />
    </span>
  )
}

function ConnRow({
  title,
  detail,
  avatar,
  gone,
  onClick,
}: {
  title: string
  detail: string
  avatar: React.ReactNode
  gone?: boolean
  onClick?: () => void
}): React.JSX.Element {
  const inner = (
    <>
      {avatar}
      <span className={`min-w-0 flex-1 ${gone ? 'opacity-60' : ''}`}>
        <span
          className={`block text-[11px] truncate ${gone ? 'text-[var(--color-text-muted)] line-through' : 'text-[var(--color-text-secondary)]'}`}
        >
          {title}
        </span>
        <span
          className={`block text-[9px] text-[var(--color-text-muted)] ${gone ? 'leading-relaxed' : 'truncate'}`}
          title={detail}
        >
          {detail}
        </span>
      </span>
    </>
  )
  const goneAttr = gone ? 'true' : undefined
  if (onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        data-gone={goneAttr}
        className="w-full flex items-center gap-2 text-left cursor-pointer no-drag hover:opacity-80"
      >
        {inner}
      </button>
    )
  }
  return (
    <div className="flex items-center gap-2" data-gone={goneAttr}>
      {inner}
    </div>
  )
}
