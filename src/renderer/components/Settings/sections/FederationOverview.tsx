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
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  listAllRemoteConnections,
  removeRemoteConnection,
  remoteConnectionGoneNote,
  type AggregatedRemoteConnection,
} from '@/lib/federation'
import { Button, Callout, Surface } from '@/components/ui'
import { ConnectionRow, SectionHeader, ServerAddress, StateNote, StatusChip } from './ConnectionsParts'

export function FederationOverview({
  /** Bump after pairing / connection edits so the list reloads. */
  refreshKey = 0,
}: {
  refreshKey?: number
} = {}): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const isLocal = activeHost === 'local'
  const hostKey = isLocal ? 'local' : activeHost.id
  const hostLabel = isLocal ? 'this computer' : activeHost.label || activeHost.hostname

  const [loading, setLoading] = useState(true)
  const [conns, setConns] = useState<AggregatedRemoteConnection[]>([])
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const [removingKey, setRemovingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  /** Couldn't read the list at all: not the same as "no agents". */
  const [loadError, setLoadError] = useState<string | null>(null)

  const load = useCallback(async (isCancelled: () => boolean = () => false): Promise<void> => {
    setLoading(true)
    setLoadError(null)
    try {
      const list = await listAllRemoteConnections()
      if (isCancelled()) return
      setConns(list)
    } catch (e) {
      if (isCancelled()) return
      setConns([])
      setLoadError(e instanceof Error ? e.message : String(e))
    } finally {
      if (!isCancelled()) setLoading(false)
    }
  }, [])

  const reload = useCallback(async () => {
    setError(null)
    await load()
  }, [load])

  useEffect(() => {
    let cancelled = false
    void load(() => cancelled)
    return () => {
      cancelled = true
    }
  }, [load, hostKey, refreshKey])

  const handleRemove = async (c: AggregatedRemoteConnection): Promise<void> => {
    const name = c.agent || c.address
    const ok = await confirm({
      title: c.host ? `Remove ${name} on ${c.host}?` : `Remove ${name}?`,
      message: `${c.sourceWorkspace} will no longer be connected to ${name}. The server stays paired, so you can add the agent again from the workspace’s Connections tab.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
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
    <section data-settings-id="connections.federation-overview">
      <SectionHeader
        title="Agents on other servers"
        hint={
          <>
            Agents on other servers connected to workspaces on{' '}
            <span className="text-[var(--color-text-primary)]">{hostLabel}</span>. Removing one here doesn&apos;t unpair
            its server.
          </>
        }
      />
      {error && (
        <Callout role="alert" className="mb-3 type-hint">
          {error}
        </Callout>
      )}
      <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
        {loading ? (
          <StateNote title="Loading…" />
        ) : loadError ? (
          <StateNote
            data-testid="connections-agents-error"
            title="Couldn’t load agents on other servers."
            hint={loadError}
            action={
              <Button variant="quiet" onClick={() => void reload()}>
                Try again
              </Button>
            }
          />
        ) : conns.length === 0 ? (
          <StateNote
            title="No agents connected yet."
            hint={<>Pair a server, then add agents from a workspace&apos;s Connections tab.</>}
          />
        ) : (
          conns.map((c) => {
            const key = `${c.sourcePath}|${c.address}`
            const busy = removingKey === key
            return (
              <ConnectionRow
                key={key}
                data-agent-row={c.address}
                // The same two lines as a server row: the agent and its
                // server's address, then its state as a chip and the
                // workspace here it's connected to.
                title={
                  <>
                    <span className="truncate flex-shrink-0 max-w-[60%]" title={c.address}>
                      {c.agent || c.address}
                    </span>
                    {c.host ? <ServerAddress>{c.host}</ServerAddress> : null}
                  </>
                }
                meta={
                  <>
                    {c.gone ? (
                      <StatusChip kind="health" tone="warn" title={remoteConnectionGoneNote(c.host)}>
                        Gone
                      </StatusChip>
                    ) : (
                      <StatusChip kind="health" tone="ok" title={`Connected to a workspace on ${hostLabel}`}>
                        Connected
                      </StatusChip>
                    )}
                    <span className="ml-0.5 min-w-0 truncate">
                      Workspace <span className="text-[var(--color-text-primary)]">{c.sourceWorkspace}</span>
                    </span>
                  </>
                }
                actions={
                  <Button
                    variant="quiet"
                    disabled={busy || removingKey !== null}
                    onClick={() => void handleRemove(c)}
                    title="Remove this connection. The server stays paired."
                  >
                    {busy ? 'Removing…' : 'Remove'}
                  </Button>
                }
              >
                {c.gone && (
                  <div className="type-hint text-[var(--color-text-secondary)]" data-gone="true">
                    {remoteConnectionGoneNote(c.host)}
                  </div>
                )}
              </ConnectionRow>
            )
          })
        )}
      </Surface>
    </section>
  )
}
