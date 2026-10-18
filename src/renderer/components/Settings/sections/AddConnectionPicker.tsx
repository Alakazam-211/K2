// AddConnectionPicker — the one "Add connection" picker on a workspace's
// Settings → Connections (0.46.1, eval-connections-pickers-v1 option b).
//
// It replaces the two flows (a local search list, and pick a federated
// server, then pick one of its agents) with ONE searchable list built on
// the Home picker's `SearchableAgentList`: a section for This server, then
// one for each trusted paired server.
//
// Daemon-first: `GET /cli/federation/peer-rosters` sends the sections in
// order (This server first, then A to Z), each server's agents A to Z and
// how each server answered (ok / offline / rejected / error, 5 s cap each).
// This file only renders that and filters it for the search box (every
// token of the query in the name, handle, aliases, server or host).
//
// - Offline / refusing servers are greyed with a Retry (that server only).
// - A server paired with the older code shows an inline re-pair hint.
// - Agents already connected stay in the list, checked.
// - Adding keeps the picker open; the row turns checked once the
//   connection lists reload. Errors show in the footer.
// - Esc closes (the caller's `onClose`).
//
// While the daemon answers (at most ~5 s), This server paints at once
// from the projects store; so does it when federation is off here or the
// daemon predates the route.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useHomeAvatarStore } from '@/lib/home-avatars'
import { isAirgap } from '@/lib/airgap'
import {
  REPAIR_RECOMMENDED_HINT,
  compareShown,
  emptyRosterNote,
  federatedPeerHost,
  fetchConnectionServers,
  formatAgentHost,
  type ConnectionServer,
  type ConnectionServersResult,
} from '@/lib/federation'
import {
  SearchableAgentList,
  tokenAndMatch,
  type AgentListRow,
  type AgentListSection,
} from '@/components/ui/SearchableAgentList'
import { AgentListStatus } from '@/components/ui/AgentListStatus'

export interface AddConnectionPickerProps {
  /** The workspace we add connections FOR (hidden from This server). */
  projectId: string
  /** Local workspace ids it already connects to. */
  connectedLocalIds: ReadonlySet<string>
  /** Remote `agent::host` addresses it already connects to (lowercase). */
  connectedRemoteAddrs: ReadonlySet<string>
  /** Add a local connection. Throws to report an error. */
  onAddLocal: (targetProjectId: string) => Promise<void>
  /** Add a federated connection to `agent::host`. Throws to report an
   *  error; a returned string is a warning to show. */
  onAddRemote: (target: string) => Promise<string | null>
  onClose: () => void
}

type PickTarget =
  | { kind: 'local'; id: string; name: string }
  | { kind: 'peer'; target: string; name: string }

type Note = { tone: 'error' | 'muted'; text: string }

/** Avatar color for an agent on another server (Home's muted color). */
const OTHER_SERVER_COLOR = 'var(--color-text-muted)'

const panelClass = 'border border-[var(--color-border)] bg-[var(--color-bg-elevated)] mb-2 flex flex-col max-h-[60vh]'
const listClass = 'flex-1 min-h-0 overflow-y-auto py-1'

/** The host a connection to `s` is recorded under (`agent::host`). */
export function connectionHost(s: Pick<ConnectionServer, 'host' | 'subdomain' | 'base_url'>): string {
  return federatedPeerHost({ subdomain: s.subdomain ?? '', base_url: s.base_url }) || s.host
}

function offlineText(s: ConnectionServer): string {
  if (s.error && s.error.startsWith('no answer within')) return `Offline: ${s.error}.`
  return 'Offline: it didn’t answer (refused or unreachable).'
}

/** Does the query name this server (label or host), so its status line
 *  stays while searching? */
function queryNamesServer(query: string, s: ConnectionServer): boolean {
  const tokens = query.trim().toLowerCase().split(/\s+/).filter(Boolean)
  const fields = [s.label, s.host].map((f) => f.toLowerCase())
  return tokens.length > 0 && tokens.every((t) => fields.some((f) => f.includes(t)))
}

export function AddConnectionPicker({
  projectId,
  connectedLocalIds,
  connectedRemoteAddrs,
  onAddLocal,
  onAddRemote,
  onClose,
}: AddConnectionPickerProps): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const avatarCache = useHomeAvatarStore((s) => s.cached)
  const [result, setResult] = useState<ConnectionServersResult | null>(null)
  /** Per-server overrides from a Retry (loading, then its new answer). */
  const [retried, setRetried] = useState<Record<string, ConnectionServer>>({})
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set())
  const [note, setNote] = useState<Note | null>(null)
  const alive = useRef(true)
  const loadSeq = useRef(0)

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  const loadAll = useCallback(() => {
    const n = ++loadSeq.current
    setResult(null)
    setRetried({})
    void fetchConnectionServers().then((r) => {
      if (alive.current && loadSeq.current === n) setResult(r)
    })
  }, [])

  useEffect(() => loadAll(), [loadAll])

  const retry = useCallback((s: ConnectionServer) => {
    setRetried((prev) => ({ ...prev, [s.key]: { ...s, status: 'loading', error: null, agents: [] } }))
    void fetchConnectionServers(s.fingerprint ?? s.key).then((r) => {
      if (!alive.current) return
      const fresh: ConnectionServer =
        r.kind === 'ok' && r.servers[0]
          ? r.servers[0]
          : {
              ...s,
              status: 'error',
              agents: [],
              error:
                r.kind === 'error'
                  ? r.message
                  : r.kind === 'ok'
                    ? 'it is no longer a trusted paired server'
                    : r.kind === 'denied'
                      ? 'your login can’t list paired servers'
                      : 'federation is off on this server',
            }
      setRetried((prev) => ({ ...prev, [s.key]: fresh }))
    })
  }, [])

  // This server: the daemon's list when it sent one, else the projects
  // store (same order rules) while it answers or when it can't.
  const localServer = useMemo((): ConnectionServer => {
    const fromDaemon = result?.kind === 'ok' ? result.servers.find((s) => s.kind === 'local') : undefined
    if (fromDaemon) return fromDaemon
    const agents = projects
      .map((p) => {
        const handle = (p.handle ?? '').trim().toLowerCase() || p.name.trim().toLowerCase()
        return { workspace_id: p.id, workspace_name: p.name, agent: handle, address: handle, aliases: [] }
      })
      .sort(
        (a, b) =>
          compareShown(a.workspace_name, b.workspace_name) || compareShown(a.agent, b.agent) || compareShown(a.address, b.address),
      )
    return { kind: 'local', key: 'local', label: 'This server', host: '', status: 'ok', error: null, agents }
  }, [result, projects])

  const peers = useMemo((): ConnectionServer[] => {
    if (result?.kind !== 'ok') return []
    return result.servers.filter((s) => s.kind === 'peer').map((s) => retried[s.key] ?? s)
  }, [result, retried])

  const projectsById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects])
  const hasQuery = query.trim().length > 0

  const picks = useRef(new Map<string, PickTarget>())
  const sections = useMemo((): AgentListSection[] => {
    const map = new Map<string, PickTarget>()
    const out: AgentListSection[] = []

    // ── This server ──
    const localRows: AgentListRow[] = []
    for (const a of localServer.agents) {
      if (a.workspace_id === projectId) continue
      const p = projectsById.get(a.workspace_id)
      const value = `local:${a.workspace_id}`
      const name = a.workspace_name || a.agent
      const taken = connectedLocalIds.has(a.workspace_id)
      map.set(value, { kind: 'local', id: a.workspace_id, name })
      localRows.push({
        value,
        label: name,
        detail: a.agent,
        avatar: {
          kind: 'workspace',
          path: p?.path ?? `conn-local:${a.workspace_id}`,
          name,
          color: p?.color || 'var(--color-neutral)',
          id: a.workspace_id,
          iconUrl: p?.iconUrl ?? null,
          fetchIcon: Boolean(p),
        },
        state: taken ? 'checked' : busy.has(value) ? 'disabled' : 'pickable',
        title: taken ? `Already connected to ${name}` : `Connect to ${name}`,
        keywords: [a.agent, p?.path ?? '', 'this server'],
      })
    }
    let localStatus: React.ReactNode = undefined
    if (localServer.status !== 'ok') {
      localStatus = <AgentListStatus tone="error" marker="error" text={`Couldn’t list this server’s agents: ${localServer.error ?? 'unknown error'}`} />
    } else if (!hasQuery && localRows.length === 0) {
      localStatus = <AgentListStatus text="No other agents on this server" />
    } else if (!hasQuery && localRows.every((r) => r.state === 'checked')) {
      localStatus = <AgentListStatus text="Every agent here is already connected" />
    }
    out.push({ key: 'local', label: 'This server', accent: true, rows: localRows, status: localStatus })

    // ── Paired servers ──
    if (result === null) {
      out.push({
        key: 'peers-pending',
        label: 'Paired servers',
        count: null,
        status: hasQuery ? undefined : <AgentListStatus marker="loading" text="Asking paired servers… (5 s at most each)" />,
        rows: [],
      })
    } else if (result.kind === 'federation-off') {
      out.push({
        key: 'peers-off',
        label: 'Paired servers',
        count: null,
        status: hasQuery ? undefined : (
          <AgentListStatus
            marker="federation-off"
            text={`Federation is off on this server. Turn it on under ${isAirgap() ? 'Servers → Policies' : 'Tunnel → Policies'} to connect to agents on paired servers.`}
          />
        ),
        rows: [],
      })
    } else if (result.kind === 'denied') {
      out.push({
        key: 'peers-denied',
        label: 'Paired servers',
        count: null,
        status: hasQuery ? undefined : (
          <AgentListStatus marker="no-access" text="Your login can’t list paired servers’ agents. An owner or Admin can add these connections." />
        ),
        rows: [],
      })
    } else if (result.kind === 'error') {
      out.push({
        key: 'peers-error',
        label: 'Paired servers',
        count: null,
        status: (
          <AgentListStatus
            marker="error"
            tone="error"
            text={`Couldn’t list paired servers: ${result.message}`}
            action={{ label: 'Retry', onClick: loadAll }}
          />
        ),
        rows: [],
      })
    } else if (peers.length === 0) {
      out.push({
        key: 'peers-none',
        label: 'Paired servers',
        count: null,
        status: hasQuery ? undefined : (
          <AgentListStatus marker="none" text="No paired servers yet. Pair one under Settings → Connected Servers (Pair as federated peer)." />
        ),
        rows: [],
      })
    }

    for (const s of peers) {
      const host = connectionHost(s)
      const rows: AgentListRow[] = []
      for (const a of s.agents) {
        const value = `peer:${s.key}:${a.workspace_id}:${a.agent}`
        const name = a.workspace_name?.trim() || a.agent
        const target = formatAgentHost(a.agent, host)
        const taken = connectedRemoteAddrs.has(target.toLowerCase())
        map.set(value, { kind: 'peer', target, name })
        const cached = avatarCache[target]?.dataUrl ?? null
        rows.push({
          value,
          label: name,
          detail: a.agent,
          avatar: {
            kind: 'workspace',
            // Its own cache key: this path means nothing on this server.
            path: `conn-peer:${s.key}:${a.workspace_id}`,
            name,
            color: OTHER_SERVER_COLOR,
            id: a.workspace_id,
            iconUrl: cached && cached.startsWith('data:image/') ? cached : null,
            fetchIcon: false,
          },
          state: taken ? 'checked' : busy.has(value) ? 'disabled' : 'pickable',
          title: taken ? `Already connected to ${target}` : `Connect to ${target}`,
          keywords: [a.agent, ...(a.aliases ?? []), s.label, host],
        })
      }
      const showStatus = !hasQuery || queryNamesServer(query, s)
      let status: React.ReactNode = undefined
      if (showStatus) {
        switch (s.status) {
          case 'loading':
            status = <AgentListStatus marker="loading" text={`Asking ${s.label}…`} />
            break
          case 'offline':
            status = <AgentListStatus marker="offline" text={offlineText(s)} action={{ label: 'Retry', onClick: () => retry(s) }} />
            break
          case 'rejected':
            status = (
              <AgentListStatus
                marker="rejected"
                text={`${s.label} refused this server: it may no longer trust it. Check the pairing under Settings → Connected Servers.`}
                action={{ label: 'Retry', onClick: () => retry(s) }}
              />
            )
            break
          case 'error':
            status = (
              <AgentListStatus
                marker="error"
                tone="error"
                text={`Couldn’t list agents: ${s.error ?? 'unknown error'}`}
                action={{ label: 'Retry', onClick: () => retry(s) }}
              />
            )
            break
          case 'ok':
            if (hasQuery) break
            if (rows.length === 0) status = <AgentListStatus marker="empty" text={emptyRosterNote(true, s.label)} />
            else if (rows.every((r) => r.state === 'checked'))
              status = <AgentListStatus text="Every agent here is already connected" />
            break
        }
      }
      out.push({
        key: `peer:${s.key}`,
        label: s.label,
        sub: host || null,
        count: s.status === 'ok' ? undefined : null,
        dim: s.status === 'offline' || s.status === 'rejected',
        headerExtra: s.repair_recommended ? (
          <span className="text-[10px] text-[var(--color-status-warn-text)] truncate" title={REPAIR_RECOMMENDED_HINT} data-repair-hint="">
            ⚠ older pairing ·{' '}
            <button
              type="button"
              className="no-drag underline cursor-pointer hover:text-[var(--color-text-primary)]"
              onClick={() => useSettingsStore.getState().setSection('connections')}
            >
              Re-pair
            </button>
          </span>
        ) : undefined,
        status,
        rows,
      })
    }
    picks.current = map
    return out
  }, [
    localServer,
    peers,
    result,
    projectId,
    projectsById,
    connectedLocalIds,
    connectedRemoteAddrs,
    busy,
    hasQuery,
    query,
    avatarCache,
    retry,
    loadAll,
  ])

  const onPick = (value: string): void => {
    const pick = picks.current.get(value)
    if (!pick || busy.has(value)) return
    setBusy((prev) => new Set(prev).add(value))
    setNote(null)
    const done = (): void => {
      if (!alive.current) return
      setBusy((prev) => {
        const next = new Set(prev)
        next.delete(value)
        return next
      })
    }
    const run = pick.kind === 'local' ? onAddLocal(pick.id).then(() => null) : onAddRemote(pick.target)
    void run
      .then((warning) => {
        if (alive.current && warning) setNote({ tone: 'muted', text: warning })
      })
      .catch((e: unknown) => {
        if (alive.current) setNote({ tone: 'error', text: `Couldn’t connect to ${pick.name}: ${e instanceof Error ? e.message : String(e)}` })
      })
      .finally(done)
  }

  return (
    <div className={panelClass} role="dialog" aria-label="Add connection">
      <SearchableAgentList
        sections={sections}
        query={query}
        onQuery={setQuery}
        onPick={onPick}
        matches={tokenAndMatch}
        placeholder="Search agents on this server and paired servers…"
        emptyText="No agents match"
        ariaLabel="Agents you can connect to"
        listClassName={listClass}
        onEscape={onClose}
        autoFocus
      />
      {note && (
        <div
          className={`px-3 py-2 text-[10px] border-t border-[var(--color-border)] ${
            note.tone === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-muted)]'
          }`}
          role={note.tone === 'error' ? 'alert' : 'status'}
          data-picker-note={note.tone}
        >
          {note.text}
        </div>
      )}
      <div className="px-3 py-1.5 text-[10px] text-[var(--color-text-muted)] border-t border-[var(--color-border)]">
        ↑/↓ move · Enter connects · Esc closes · stays open after each add
      </div>
    </div>
  )
}
