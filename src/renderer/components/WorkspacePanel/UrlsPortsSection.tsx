import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useServerSupports } from '@/lib/server-capabilities'
import { useTunnelUrls } from '@/hooks/useTunnelUrls'
import { onAppHello, onPublishServicesChanged } from '@/stores/session-events'
import {
  PUBLISH_RUN_EXAMPLE,
  byoWorkspaceTargets,
  isServiceStoppable,
  nestedPublicUrl,
  parsePublishList,
  serviceListenLabel,
  servicePublicUrl,
  sortedServices,
  sortedTargets,
  unattributedCount,
  unattributedHint,
  workspaceTargets,
  type PublishedService,
} from './urls-ports'

// Published — a collapsible Workspace-drawer section showing THIS
// workspace's daemon-owned hosted services plus leftover BYO nested
// URLs (`k2 publish subdomain create` with no matching service name).
// Deliberately NOT the server-wide view — no primary-tunnel details,
// no other workspaces' rows; that generic surface lives in Settings →
// K2 Connect (TunnelUrlsPanel). Nested-URL state is shared with that
// panel via `useTunnelUrls`. Services come from GET /cli/publish/list
// when the daemon speaks `publish-services`; older remotes hide
// Start/Stop and still show attributed nested URLs.

function statusDotClass(status: string): string {
  if (status === 'running') return 'bg-[var(--color-status-success)]'
  if (status === 'starting') return 'bg-[var(--color-status-working)]'
  if (status === 'exited' || status === 'unhealthy') return 'bg-[var(--color-status-error)]'
  return 'bg-[var(--color-text-muted)]'
}

export function UrlsPortsSection({ projectId }: { projectId: string }): React.JSX.Element {
  const { status, subs } = useTunnelUrls()
  const supportsPublish = useServerSupports('publish-services')

  // Collapse state, persisted per-workspace (the Worktrees idiom).
  const collapseKey = projectId ? `urls-ports.section-collapsed.${projectId}` : null
  const [open, setOpen] = useState<boolean>(() => {
    if (!collapseKey) return true
    return localStorage.getItem(collapseKey) !== 'closed'
  })
  useEffect(() => {
    if (!collapseKey) {
      setOpen(true)
      return
    }
    setOpen(localStorage.getItem(collapseKey) !== 'closed')
  }, [collapseKey])
  const toggle = useCallback((): void => {
    setOpen((cur) => {
      const next = !cur
      if (collapseKey) localStorage.setItem(collapseKey, next ? 'open' : 'closed')
      return next
    })
  }, [collapseKey])

  const [services, setServices] = useState<PublishedService[] | undefined>(
    supportsPublish ? undefined : [],
  )
  const [busyName, setBusyName] = useState<string | null>(null)

  useEffect(() => {
    if (!supportsPublish || !projectId) {
      setServices([])
      return
    }
    let cancelled = false
    setServices(undefined)
    const refresh = async (): Promise<void> => {
      try {
        const raw = await daemonCliGet<unknown>('publish/list', { project: projectId })
        if (!cancelled) setServices(parsePublishList(raw))
      } catch {
        if (!cancelled) setServices((prev) => prev ?? [])
      }
    }
    void refresh()
    const offHello = onAppHello(() => {
      void refresh()
    })
    const offChanged = onPublishServicesChanged((e) => {
      if (e.projectId && e.projectId !== projectId) return
      void refresh()
    })
    return () => {
      cancelled = true
      offHello()
      offChanged()
    }
  }, [projectId, supportsPublish])

  const act = useCallback(
    async (name: string, action: 'start' | 'stop'): Promise<void> => {
      if (!projectId) return
      setBusyName(name)
      try {
        await daemonCliPost(`publish/${action}`, { name, project: projectId })
        const raw = await daemonCliGet<unknown>('publish/list', { project: projectId })
        setServices(parsePublishList(raw))
      } catch {
        /* list refresh / next event converges; keep the drawer quiet */
      } finally {
        setBusyName(null)
      }
    },
    [projectId],
  )

  const publicUrl = status?.public_url ?? null
  const allTargets = subs && typeof subs === 'object' ? subs.targets : {}
  const primary = subs && typeof subs === 'object' ? subs.primary : ''
  const mine = workspaceTargets(allTargets, projectId)
  const serviceList = services ?? []
  const byo = byoWorkspaceTargets(mine, serviceList)
  const serviceRows = sortedServices(serviceList)
  const nestedRows = sortedTargets(byo)
  const claimable = unattributedCount(allTargets)
  const hint = unattributedHint(claimable)
  const hasAny = serviceRows.length + nestedRows.length > 0
  const rowCount = serviceRows.length + nestedRows.length
  const loadingServices = supportsPublish && services === undefined
  const loadingSubs = subs === undefined
  const unavailable = !supportsPublish && subs === null

  return (
    <>
      {/* ── Header (the Worktrees collapsible idiom) ── */}
      <button
        onClick={toggle}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={open ? 'Collapse Published' : 'Expand Published'}
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
          {/* Globe glyph — accent-tinted to match the section-header
              weight of the Heartbeats/Worktrees icons. */}
          <svg
            className="w-3 h-3 text-[var(--color-accent)] flex-shrink-0"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="8" cy="8" r="6.5" />
            <path d="M1.5 8h13M8 1.5c1.8 1.7 2.8 4 2.8 6.5S9.8 13.3 8 14.5C6.2 12.8 5.2 10.5 5.2 8S6.2 3.2 8 1.5z" />
          </svg>
          Published
          {rowCount > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {rowCount}
            </span>
          )}
        </span>
      </button>

      {open && (
        <div className="px-3 py-2 border-b border-[var(--color-border)]">
          {(loadingSubs || loadingServices) && !hasAny ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Checking Published…
            </p>
          ) : unavailable && !hasAny ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Not available on this daemon.
            </p>
          ) : !hasAny ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Ask your agent to publish it with{' '}
              <span className="font-mono">{PUBLISH_RUN_EXAMPLE}</span>
            </p>
          ) : (
            <div className="space-y-1.5">
              {serviceRows.map((svc) => {
                const listen = serviceListenLabel(svc)
                const url = servicePublicUrl(svc)
                const stoppable = isServiceStoppable(svc)
                const busy = busyName === svc.name
                return (
                  <div key={`svc:${svc.name}`} className="min-w-0">
                    <div className="flex items-center gap-1.5 min-w-0">
                      <span
                        className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${statusDotClass(svc.status)}`}
                        title={svc.status || 'unknown'}
                      />
                      <span className="text-[11px] text-[var(--color-text-primary)] truncate flex-1">
                        {svc.name}
                      </span>
                      {supportsPublish && (
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => void act(svc.name, stoppable ? 'stop' : 'start')}
                          className="text-[9px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50 no-drag flex-shrink-0"
                        >
                          {stoppable ? 'Stop' : 'Start'}
                        </button>
                      )}
                    </div>
                    {listen ? (
                      <span className="block text-[10px] font-mono text-[var(--color-text-muted)] truncate">
                        {listen}
                      </span>
                    ) : null}
                    {url ? (
                      <a
                        href={url}
                        target="_blank"
                        rel="noreferrer"
                        className="block text-[11px] font-mono text-[var(--color-accent)] hover:underline truncate no-drag cursor-pointer"
                      >
                        {url}
                      </a>
                    ) : null}
                    {svc.error ? (
                      <span className="block text-[10px] text-[var(--color-status-error)] truncate" title={svc.error}>
                        {svc.error}
                      </span>
                    ) : null}
                  </div>
                )
              })}
              {nestedRows.map(([label, info]) => {
                const url = nestedPublicUrl(label, primary, publicUrl)
                return (
                  <div key={`byo:${label}`} className="min-w-0">
                    {url ? (
                      <a
                        href={url}
                        target="_blank"
                        rel="noreferrer"
                        className="block text-[11px] font-mono text-[var(--color-accent)] hover:underline truncate no-drag cursor-pointer"
                      >
                        {url}
                      </a>
                    ) : (
                      <span className="block text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                        {label}
                      </span>
                    )}
                    <span className="block text-[10px] text-[var(--color-text-muted)] truncate">
                      → {info.target}
                    </span>
                  </div>
                )
              })}
            </div>
          )}
          {hint ? (
            <p className="text-[10px] text-[var(--color-text-muted)] mt-1.5">
              {hint}
            </p>
          ) : null}
        </div>
      )}
    </>
  )
}
