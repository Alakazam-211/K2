import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { activateProject, useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { SectionManageCog } from './SectionManageCog'

/** One row from GET `/cli/connections?action=list` — present edges only. */
type ConnListRow = {
  remote?: boolean
  projectId?: string
  projectName?: string
  path?: string
  reachable?: boolean
  address?: string
  host?: string
  agent?: string
}

function parseConnectionRows(body: unknown): ConnListRow[] {
  let value: unknown = body
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value)
    } catch {
      return []
    }
  }
  if (!value || typeof value !== 'object') return []
  const rows = (value as { connections?: unknown }).connections
  return Array.isArray(rows) ? (rows as ConnListRow[]) : []
}

/** Collapsible Connected Agents — only workspaces with a present connection. */
export function ConnectedAgentsSection({ projectId }: { projectId: string }): React.JSX.Element {
  const collapseKey = `connected-agents.section-collapsed.${projectId}`
  const [open, setOpen] = useState<boolean>(() => localStorage.getItem(collapseKey) !== 'closed')
  const [rows, setRows] = useState<ConnListRow[]>([])
  const [loaded, setLoaded] = useState(false)
  const project = useProjectsStore((s) => s.projects.find((p) => p.id === projectId) ?? null)
  const projects = useProjectsStore((s) => s.projects)
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
        const body = await daemonCliGet<unknown>('connections', {
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
          >
            <circle cx="5" cy="8" r="2" />
            <circle cx="11" cy="5" r="2" />
            <circle cx="11" cy="11" r="2" />
            <path d="M6.7 7.2 L9.3 5.8M6.7 8.8 L9.3 10.2" />
          </svg>
          Connected Agents
          {count > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {count}
            </span>
          )}
        </span>
        <SectionManageCog
          title="Manage connections"
          onClick={() => {
            useSettingsStore.getState().openSettings('projects', projectId, 'connections')
          }}
        />
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
                const title = row.agent || row.address || 'Remote agent'
                const detail = row.host || row.address || 'federated'
                return (
                  <ConnRow
                    key={row.address || `remote-${i}`}
                    title={title}
                    detail={detail}
                    federated
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
  onClick,
}: {
  title: string
  detail: string
  color?: string | null
  federated?: boolean
  onClick?: () => void
}): React.JSX.Element {
  const inner = (
    <>
      <span
        className="w-2 h-2 flex-shrink-0 rounded-full"
        style={{
          backgroundColor: federated
            ? 'var(--color-accent)'
            : color || 'var(--color-neutral)',
        }}
      />
      <span className="min-w-0 flex-1">
        <span className="block text-[11px] text-[var(--color-text-secondary)] truncate">
          {title}
        </span>
        <span className="block text-[9px] text-[var(--color-text-muted)] truncate">
          {federated ? `federated · ${detail}` : detail}
        </span>
      </span>
    </>
  )
  if (onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        className="w-full flex items-center gap-2 text-left cursor-pointer no-drag hover:opacity-80"
      >
        {inner}
      </button>
    )
  }
  return <div className="flex items-center gap-2">{inner}</div>
}
