import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { activateProject, useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { SectionManageCog } from './SectionManageCog'
import { scopedKey } from '@/kessel/server-scope'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import { parseConnectionRows, remoteConnRowView, type ConnListRow } from './connected-agents-rows'

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
          <svg
            className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden
          >
            <circle cx="8" cy="2.5" r="1" />
            <path d="M8 3.5v1.5" />
            <rect x="3" y="5" width="10" height="9" rx="2" />
            <circle cx="6.25" cy="9" r="1" />
            <circle cx="9.75" cy="9" r="1" />
            <path d="M6.5 12h3" />
          </svg>
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
                    federated
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
                  color={local?.color}
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

function ConnRow({
  title,
  detail,
  color,
  federated,
  gone,
  onClick,
}: {
  title: string
  detail: string
  color?: string | null
  federated?: boolean
  gone?: boolean
  onClick?: () => void
}): React.JSX.Element {
  const inner = (
    <>
      <span
        className="w-2 h-2 flex-shrink-0"
        style={{
          backgroundColor: gone
            ? 'var(--color-neutral)'
            : federated
              ? 'var(--color-accent)'
              : color || 'var(--color-neutral)',
        }}
      />
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
