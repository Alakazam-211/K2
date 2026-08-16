import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { SectionManageCog } from './SectionManageCog'
import { HideApiSessionsToggle } from './HideApiSessionsToggle'
import {
  keyGrantsWorkspace,
  keyState,
  workspaceGrantSlug,
  type ApiKeyRow,
} from '@/components/Settings/sections/api-keys-api'

type ListResponse = { keys?: ApiKeyRow[] }

/** Compact Workspace-drawer API section — tokens that grant this workspace. */
export function WorkspaceApiSection({
  project,
}: {
  project: { id: string; name: string; path: string; hideApiSessions?: number }
}): React.JSX.Element {
  const slug = workspaceGrantSlug(project)
  const collapseKey = `workspace-api.section-collapsed.${project.id}`
  const [open, setOpen] = useState<boolean>(() => localStorage.getItem(collapseKey) !== 'closed')
  const [keys, setKeys] = useState<ApiKeyRow[]>([])
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState<string | null>(null)

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

  const refresh = useCallback(async () => {
    try {
      const d = await daemonCliGet<ListResponse>('api-keys/list')
      const all = Array.isArray(d.keys) ? d.keys : []
      setKeys(all.filter((k) => keyGrantsWorkspace(k, slug)))
    } catch {
      setKeys([])
    } finally {
      setLoading(false)
    }
  }, [slug])

  useEffect(() => {
    setLoading(true)
    void refresh()
  }, [refresh])

  const act = useCallback(
    async (id: string, action: 'disable' | 'enable') => {
      setBusyId(id)
      try {
        await daemonCliPost(`api-keys/${action}`, { id })
        await refresh()
      } catch {
        /* list refresh surfaces emptiness; keep drawer quiet */
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  const activeCount = keys.filter((k) => keyState(k) === 'active').length

  return (
    <>
      <button
        onClick={toggle}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={open ? 'Collapse API' : 'Expand API'}
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
            <path d="M3 8h10M8 3v10" />
            <rect x="2.5" y="2.5" width="11" height="11" rx="1.5" />
          </svg>
          API
          {activeCount > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {activeCount}
            </span>
          )}
        </span>
        <SectionManageCog
          title="Manage API keys"
          onClick={() => {
            useSettingsStore.getState().openSettings('projects', project.id, 'api')
          }}
        />
      </button>

      {open && (
        <div className="px-3 py-2 border-b border-[var(--color-border)] space-y-2">
          <HideApiSessionsToggle project={project} />
          {loading ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">Loading…</p>
          ) : keys.length === 0 ? (
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              No API keys grant this workspace.
            </p>
          ) : (
            keys.map((k) => {
              const state = keyState(k)
              const busy = busyId === k.id
              return (
                <div key={k.id} className="flex items-center gap-2 min-w-0">
                  <span
                    className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${
                      state === 'active'
                        ? 'bg-[var(--color-status-success)]'
                        : state === 'disabled'
                          ? 'bg-[var(--color-text-muted)]'
                          : 'bg-[var(--color-status-error-soft)]'
                    }`}
                  />
                  <span className="text-[11px] text-[var(--color-text-primary)] truncate flex-1">
                    {k.label || '(no label)'}
                  </span>
                  {state === 'active' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'disable')}
                      className="text-[9px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50 no-drag"
                    >
                      off
                    </button>
                  )}
                  {state === 'disabled' && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void act(k.id, 'enable')}
                      className="text-[9px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50 no-drag"
                    >
                      on
                    </button>
                  )}
                </div>
              )
            })
          )}
        </div>
      )}
    </>
  )
}
