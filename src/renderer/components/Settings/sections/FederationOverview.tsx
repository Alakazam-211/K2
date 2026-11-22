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
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import {
  listAllRemoteConnections,
  removeRemoteConnection,
  type AggregatedRemoteConnection,
} from '@/lib/federation'
import { Button, Callout } from '@/components/ui'
import { SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { ConnectionRow, Facts, ProblemNote, ServerAddress, StatusDot, StatusFact } from './ConnectionsParts'
import { PRODUCT_NAME } from '@/lib/brand'

/** Why a gone agent stopped working, in plain words. */
function goneLine(host: string): string {
  return `Its workspace is no longer on ${host || 'that server'}.`
}

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
  const projects = useProjectsStore((s) => s.projects)
  /** A workspace's Connections tab, where a gone agent is added again. */
  const openConnectionsTab = (sourcePath: string): void => {
    const id = projects.find((p) => p.path === sourcePath)?.id
    if (id) useSettingsStore.getState().openSettings('projects', id, 'connections')
  }
  const [removingKey, setRemovingKey] = useState<string | null>(null)
  /** The last Remove that failed, on its own row. */
  const [error, setError] = useState<{ key: string; message: string } | null>(null)
  /** The last Remove that worked, under the list. */
  const [removed, setRemoved] = useState<string | null>(null)
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
    setRemoved(null)
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
      title: `Remove ${name} from ${c.sourceWorkspace}?`,
      message: `${c.sourceWorkspace} stops being able to message ${name}${c.host ? ` on ${c.host}` : ''}. The server stays paired, so you can add the agent again from the workspace’s Connections tab.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    const key = `${c.sourcePath}|${c.address}`
    setRemovingKey(key)
    setError(null)
    setRemoved(null)
    try {
      await removeRemoteConnection(c.sourcePath, c.address)
      await load()
      setRemoved(`Removed ${name} from ${c.sourceWorkspace}.`)
    } catch (e) {
      setError({ key, message: e instanceof Error ? e.message : String(e) })
    } finally {
      setRemovingKey(null)
    }
  }

  return (
    <div data-settings-id="connections.federation-overview">
      <SettingsGroup
        card
        title="Agents on other servers"
        hint={
          <>
            Agents on other servers that workspaces on{' '}
            <span className="text-[var(--color-text-primary)]">{hostLabel}</span> can message. Removing one leaves
            its server paired.
          </>
        }
        state={
          loading ? (
            <SettingsLoadState kind="loading" title="Loading agents…" />
          ) : loadError ? (
            <SettingsLoadState
              kind="error"
              testId="connections-agents-error"
              title="Couldn’t load agents on other servers"
              detail={loadError}
              action={<Button onClick={() => void reload()}>Try again</Button>}
            >
              {PRODUCT_NAME} couldn’t read this computer’s workspaces.
            </SettingsLoadState>
          ) : conns.length === 0 ? (
            <SettingsLoadState kind="empty" title="No agents connected yet">
              Pair a server, then add its agents from a workspace’s Connections tab.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {conns.map((c) => {
          const key = `${c.sourcePath}|${c.address}`
          const busy = removingKey === key
          return (
            <ConnectionRow
              key={key}
              data-agent-row={c.address}
              mark={<StatusDot tone={c.gone ? 'warn' : 'ok'} />}
              // The same two lines as a server row: the agent and its
              // server's address, then its state and the workspace here
              // it's connected to.
              title={
                <>
                  <span className="truncate flex-shrink-0 max-w-[60%]" title={c.address}>
                    {c.agent || c.address}
                  </span>
                  {c.host ? <ServerAddress>{c.host}</ServerAddress> : null}
                </>
              }
              meta={
                <Facts>
                  {c.gone ? (
                    <StatusFact kind="health" tone="warn" title={goneLine(c.host)}>
                      Gone
                    </StatusFact>
                  ) : (
                    <StatusFact kind="health" tone="ok" title={`Connected to a workspace on ${hostLabel}`}>
                      Connected
                    </StatusFact>
                  )}
                  <span className="min-w-0 truncate">
                    Workspace <span className="text-[var(--color-text-primary)]">{c.sourceWorkspace}</span>
                  </span>
                </Facts>
              }
              actions={
                <Button
                  variant="danger-quiet"
                  disabled={busy || removingKey !== null}
                  onClick={() => void handleRemove(c)}
                  title="Remove this connection. The server stays paired."
                  aria-label={`Remove ${c.agent || c.address} from ${c.sourceWorkspace}`}
                >
                  {busy ? 'Removing…' : 'Remove'}
                </Button>
              }
            >
              {c.gone && (
                <div className="type-hint text-[var(--color-text-secondary)] leading-relaxed" data-gone="true">
                  <p>
                    {goneLine(c.host)} Add it again from {c.sourceWorkspace}’s Connections tab.
                  </p>
                  {projects.some((p) => p.path === c.sourcePath) && (
                    <button
                      type="button"
                      data-k2-focus=""
                      data-open-connections={c.sourcePath}
                      onClick={() => openConnectionsTab(c.sourcePath)}
                      className="mt-1 type-hint text-[var(--color-accent-text)] hover:underline cursor-pointer"
                    >
                      Open {c.sourceWorkspace}’s Connections tab
                    </button>
                  )}
                </div>
              )}
              {error?.key === key && (
                <ProblemNote
                  what={`Couldn’t remove ${c.agent || c.address}.`}
                  next="Nothing changed. Try again."
                  detail={error.message}
                  testId="connections-agent-remove-error"
                />
              )}
            </ConnectionRow>
          )
        })}
      </SettingsGroup>
      {removed && (
        <Callout tone="ok" role="status" className="mt-3 type-hint flex items-center justify-between gap-3">
          <span>{removed}</span>
          <Button variant="quiet" onClick={() => setRemoved(null)}>
            OK
          </Button>
        </Callout>
      )}
    </div>
  )
}
