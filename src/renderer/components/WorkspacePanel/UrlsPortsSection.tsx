import { useCallback, useEffect, useState } from 'react'
import { useTunnelUrls } from '@/hooks/useTunnelUrls'
import {
  nestedPublicUrl,
  sortedTargets,
  unattributedCount,
  workspaceTargets,
} from './urls-ports'

// URLs — a collapsible Workspace-drawer section showing THIS workspace's
// nested K2 Connect URLs only: the labels whose 0074 attribution row
// points at this project (`k2 publish subdomain create/point/claim` from
// the workspace stamps it). Deliberately NOT the server-wide view — no
// primary-tunnel details, no other workspaces' rows; that generic surface
// lives in Settings → K2 Connect (TunnelUrlsPanel). Shares its
// fetch/subscribe state with that panel via `useTunnelUrls` (live via the
// `tunnel_status_changed` + `tunnel_subdomains_changed` broadcasts, poll
// fallback for older daemons).

export function UrlsPortsSection({ projectId }: { projectId: string }): React.JSX.Element {
  const { status, subs } = useTunnelUrls()

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

  const publicUrl = status?.public_url ?? null
  const allTargets = subs && typeof subs === 'object' ? subs.targets : {}
  const primary = subs && typeof subs === 'object' ? subs.primary : ''
  const mine = workspaceTargets(allTargets, projectId)
  const rows = sortedTargets(mine)
  const claimable = unattributedCount(allTargets)

  return (
    <>
      {/* ── Header (the Worktrees collapsible idiom) ── */}
      <button
        onClick={toggle}
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-white/[0.02] transition-colors"
        title={open ? 'Collapse URLs' : 'Expand URLs'}
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
          URLs
          {rows.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {rows.length}
            </span>
          )}
        </span>
      </button>

      {open && (
        <div className="px-3 py-2 border-b border-[var(--color-border)]">
          {subs === undefined ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Checking URLs…
            </p>
          ) : subs === null ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Not available on this daemon.
            </p>
          ) : rows.length === 0 ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Ask your agent to publish it with k2 publish
            </p>
          ) : (
            <div className="space-y-1.5">
              {rows.map(([label, info]) => {
                const url = nestedPublicUrl(label, primary, publicUrl)
                return (
                  <div key={label} className="min-w-0">
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
        </div>
      )}
    </>
  )
}
