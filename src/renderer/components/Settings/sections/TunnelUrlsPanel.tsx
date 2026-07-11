import { useProjectsStore } from '@/stores/projects'
import { useTunnelUrls } from '@/hooks/useTunnelUrls'
import {
  nestedPublicUrl,
  sortedTargets,
} from '@/components/WorkspacePanel/urls-ports'

// URLs — the SERVER-WIDE tunnel view inside Settings → K2 Connect:
// tunnel running / public URL / local port / relay plus the FULL nested
// subdomain table with its 0074 workspace-attribution column. This is the
// generic surface the workspace drawer deliberately is NOT (the drawer
// shows only the active workspace's attributed URLs); state comes from
// the same shared `useTunnelUrls` hook so both can never disagree.
// Workspace names resolve through the projects store when the id is
// registered here; a foreign/dangling id renders as the raw id — honest,
// never fabricated.

// One label/value row, matching the drawer's identity-row idiom.
function InfoRow({ label, children }: { label: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex items-baseline gap-2 mt-1 first:mt-0">
      <span className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)] flex-shrink-0 w-16">
        {label}
      </span>
      {children}
    </div>
  )
}

export function TunnelUrlsPanel(): React.JSX.Element {
  const { status, subs } = useTunnelUrls()
  const projects = useProjectsStore((s) => s.projects)

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? null
  const targets = subs && typeof subs === 'object' ? subs.targets : {}
  const primary = subs && typeof subs === 'object' ? subs.primary : ''
  const rows = sortedTargets(targets)

  const workspaceName = (projectId: string | null): string => {
    if (!projectId) return '—'
    return projects.find((p) => p.id === projectId)?.name ?? projectId
  }

  return (
    <div className="space-y-3">
      {/* ── Tunnel status ── */}
      {status === null ? (
        <p className="text-[10px] text-[var(--color-text-muted)]">
          Checking tunnel status…
        </p>
      ) : !running ? (
        <div className="flex items-center gap-2">
          <span
            className="w-2 h-2 flex-shrink-0 rounded-full"
            style={{ backgroundColor: 'var(--color-neutral)' }}
          />
          <span className="text-[11px] text-[var(--color-text-muted)]">
            K2 Connect tunnel isn&apos;t running
          </span>
        </div>
      ) : (
        <div>
          <InfoRow label="Public">
            {publicUrl ? (
              <a
                href={publicUrl}
                target="_blank"
                rel="noreferrer"
                className="text-[11px] font-mono text-[var(--color-accent)] hover:underline truncate no-drag cursor-pointer"
              >
                {publicUrl}
              </a>
            ) : (
              <span className="text-[11px] text-[var(--color-text-muted)]">
                (assigned by server)
              </span>
            )}
          </InfoRow>
          {status.subdomain && (
            <InfoRow label="Subdomain">
              <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                {status.subdomain}
              </span>
            </InfoRow>
          )}
          {typeof status.local_port === 'number' && (
            <InfoRow label="Local port">
              <span className="text-[11px] font-mono tabular-nums text-[var(--color-text-primary)]">
                {status.local_port}
              </span>
            </InfoRow>
          )}
          {status.server_addr && (
            <InfoRow label="Relay">
              <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                {status.server_addr}
              </span>
            </InfoRow>
          )}
        </div>
      )}

      {/* ── Nested subdomain table (full, server-wide) ── */}
      <div className="pt-2 border-t border-[var(--color-border)]">
        <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider">
          Nested URLs
        </span>
        {subs === undefined ? (
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1">Loading…</p>
        ) : subs === null ? (
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1">
            Not available on this daemon.
          </p>
        ) : rows.length === 0 ? (
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1">
            No nested URLs —{' '}
            <span className="font-mono">
              k2 connect subdomain create &lt;label&gt; --target localhost:&lt;port&gt;
            </span>
          </p>
        ) : (
          <div className="mt-1.5">
            <div className="grid grid-cols-[minmax(0,2fr)_minmax(0,1fr)_minmax(0,1fr)] gap-x-3 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] pb-1 border-b border-[var(--color-border)]">
              <span>URL</span>
              <span>Target</span>
              <span>Workspace</span>
            </div>
            {rows.map(([label, info]) => {
              const url = nestedPublicUrl(label, primary, publicUrl)
              return (
                <div
                  key={label}
                  className="grid grid-cols-[minmax(0,2fr)_minmax(0,1fr)_minmax(0,1fr)] gap-x-3 py-1 border-b border-[var(--color-border)] last:border-b-0"
                >
                  {url ? (
                    <a
                      href={url}
                      target="_blank"
                      rel="noreferrer"
                      className="text-[11px] font-mono text-[var(--color-accent)] hover:underline truncate no-drag cursor-pointer"
                    >
                      {url}
                    </a>
                  ) : (
                    <span className="text-[11px] font-mono text-[var(--color-text-primary)] truncate">
                      {label}
                    </span>
                  )}
                  <span className="text-[11px] font-mono text-[var(--color-text-secondary)] truncate">
                    {info.target}
                  </span>
                  <span
                    className={`text-[11px] truncate ${info.projectId ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)]'}`}
                    title={info.projectId ?? 'Not attributed to a workspace — adopt with: k2 connect subdomain claim <label>'}
                  >
                    {workspaceName(info.projectId)}
                  </span>
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
