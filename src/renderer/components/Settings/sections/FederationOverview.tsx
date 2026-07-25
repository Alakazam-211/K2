// Cross-agent connections overview for Settings → K2 Connect → Servers.
//
// Federated *server* pins are shown on each saved-server tile ("Peer: trusted"
// / Pair) — this panel no longer duplicates that list.
//
// Instead: every `agent::host` connection on the ACTIVE daemon — which remote
// agents can speak to which local workspace. Host-aware via daemonCli*.
// Remove uses existing `removeRemoteConnection` (local + best-effort reverse).

import React, { useCallback, useEffect, useState } from 'react'
import { useConnectHostStore } from '@/stores/connect-host'
import {
  listAllRemoteConnections,
  removeRemoteConnection,
  type AggregatedRemoteConnection,
} from '@/lib/federation'
import { SettingsGroup } from '../controls/SettingControls'

const BTN_DANGER =
  'px-2 py-1 text-[11px] text-[var(--color-status-error-soft)] border border-[color-mix(in_srgb,var(--color-status-error-soft)_40%,transparent)] hover:bg-[color-mix(in_srgb,var(--color-status-error-soft)_10%,transparent)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-default flex-shrink-0'

export function FederationOverview({
  /** Bump after pairing / connection edits so the list reloads. */
  refreshKey = 0,
}: {
  refreshKey?: number
} = {}): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isLocal = activeHost === 'local'
  const hostKey = isLocal ? 'local' : activeHost.id
  const hostLabel = isLocal ? 'This Mac' : activeHost.label || activeHost.hostname

  const [loading, setLoading] = useState(true)
  const [conns, setConns] = useState<AggregatedRemoteConnection[]>([])
  const [removingKey, setRemovingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    setError(null)
    const list = await listAllRemoteConnections()
    setConns(list)
    setLoading(false)
  }, [])

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    void (async () => {
      const list = await listAllRemoteConnections()
      if (cancelled) return
      setConns(list)
      setLoading(false)
    })()
    return () => {
      cancelled = true
    }
  }, [hostKey, refreshKey])

  const handleRemove = async (c: AggregatedRemoteConnection): Promise<void> => {
    const key = `${c.sourcePath}|${c.address}`
    setRemovingKey(key)
    setError(null)
    try {
      await removeRemoteConnection(c.sourcePath, c.address)
      await reload()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setRemovingKey(null)
    }
  }

  return (
    <div data-settings-id="connections.federation-overview">
      <SettingsGroup title="External agents">
        <p className="text-[10px] text-[var(--color-text-muted)] mb-2 leading-relaxed">
          Remote agents permitted to speak with workspaces on{' '}
          <span className="text-[var(--color-text-secondary)]">{hostLabel}</span>
          {' '}(<code className="text-[9px]">agent::host</code> links). Pairing a server
          (tile → Peer) is separate — that only establishes server trust. Remove drops
          this link only (not server pairing).
        </p>
        {error && (
          <div className="mb-2 text-[10px] text-[var(--color-status-error-soft)] px-2 py-1 border border-[color-mix(in_srgb,var(--color-status-error-soft)_25%,transparent)]">
            {error}
          </div>
        )}
        {loading ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2">Loading…</div>
        ) : conns.length === 0 ? (
          <div className="text-[10px] text-[var(--color-text-muted)] px-3 py-2 border border-[var(--color-border)]">
            No external agent links yet. Add connections from a workspace (Access / Connected
            workspaces) after servers are paired if needed.
          </div>
        ) : (
          <div className="space-y-2">
            {conns.map((c) => {
              const key = `${c.sourcePath}|${c.address}`
              const busy = removingKey === key
              return (
                <div
                  key={key}
                  className="flex items-center gap-2 px-3 py-2 border border-[var(--color-border)]"
                >
                  <span className="w-2 h-2 flex-shrink-0 rounded-full bg-[var(--color-accent)]" />
                  <div className="flex flex-col min-w-0 flex-1">
                    <span className="text-xs text-[var(--color-text-primary)] truncate" title={c.address}>
                      External agent{' '}
                      <span className="font-medium">{c.agent || c.address}</span>
                      {c.host ? (
                        <span className="text-[var(--color-text-muted)] font-normal">
                          {' '}on {c.host}
                        </span>
                      ) : null}
                    </span>
                    <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                      may speak with workspace{' '}
                      <span className="text-[var(--color-text-secondary)]">{c.sourceWorkspace}</span>
                    </span>
                  </div>
                  <button
                    type="button"
                    className={BTN_DANGER}
                    disabled={busy || removingKey !== null}
                    onClick={() => void handleRemove(c)}
                    title="Remove this agent↔workspace link (does not unpair the server)"
                  >
                    {busy ? 'Removing…' : 'Remove'}
                  </button>
                </div>
              )
            })}
          </div>
        )}
      </SettingsGroup>
    </div>
  )
}
