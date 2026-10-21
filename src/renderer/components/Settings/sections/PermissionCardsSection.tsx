// Settings → Access → Permission Cards (Beta), prd-agent-settings-cards-v1
// §2.1, §2.2, §8 (lane D2). Also the Agent tab's "Can ask for permission
// cards" row and its "Asked for this" chip, which ProjectsSection mounts.
//
// Daemon-first: the switches, the agent list, "Waiting on you" and the
// recent requests are all read from the daemon (permission-cards-api.ts);
// every switch is one gesture posted back, painted optimistically and
// rolled back on a refusal. The page re-reads on the daemon's
// `sync:settings` permission frames and, while visible, on a slow poll.
//
// Who sees what (PRD §2.1, §2.2):
//   - the global switch: everyone sees its state; only the owner flips it
//     (the daemon's OWNER_ONLY_KEYS; an admin or member sees it read-only);
//   - the agents, "Waiting on you" and recent requests: only while the
//     global switch is on, and only for the owner or an admin (the per-agent
//     switch's floor). A member sees one line instead;
//   - the Agent tab row: only while the global switch is on, owner/admin.

import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SettingsGroup } from '../controls/SettingControls'
import { Toggle } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { useConnectHostStore } from '@/stores/connect-host'
import { formatRelativeTime } from '@/lib/format-relative-time'
import {
  asksForRow,
  fetchPermissionAgents,
  fetchPermissionCardsSetting,
  fetchPermissionPending,
  onPermissionCardsSync,
  openPermissionCard,
  permissionErrorMessage,
  setAgentPermissionCards,
  setPermissionCardsSetting,
  type PermissionAgent,
  type PermissionPending,
  type PermissionRecent,
  type PermissionViewerRole,
} from './permission-cards-api'

export const PERMISSION_CARDS_MANIFEST: SettingEntry[] = [
  {
    id: 'permission-cards.switch',
    section: 'permission-cards',
    label: 'Permission cards (Beta)',
    description:
      'Let agents ask, with a card in their Thread, for a setting you would otherwise change in Settings. Owner only; off by default',
    keywords: ['permission cards', 'permission', 'cards', 'ask', 'request', 'approve', 'allow', 'deny', 'beta', 'k2 permission'],
  },
  {
    id: 'permission-cards.agents',
    section: 'permission-cards',
    label: 'Agents that can ask for permission cards',
    description: "Each agent's \"Can ask for permission cards\" switch, and Turn on for all agents",
    keywords: ['permission cards', 'agents', 'turn on for all', 'can ask'],
  },
  {
    id: 'permission-cards.waiting',
    section: 'permission-cards',
    label: 'Permission requests waiting on you',
    description: 'Cards agents posted that you can answer, and recent requests',
    keywords: ['waiting on you', 'pending', 'requests', 'permission cards'],
  },
]

const POLL_MS = 15_000

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

function visible(): boolean {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

/** May this viewer flip the global switch? Only the owner (§2.1). */
export function globalSwitchCanFlip(role: PermissionViewerRole | null): boolean {
  return role === 'owner'
}

/** The Agent tab row: shown only while the global switch is on (§2.2), to
 *  the owner or an admin (the route's floor); a member never sees it. */
export function agentPermissionRowVisibility(
  role: PermissionViewerRole | null,
  serverOn: boolean | null,
): { show: boolean; canFlip: boolean } {
  if (serverOn !== true || role === 'member') return { show: false, canFlip: false }
  return { show: true, canFlip: role === 'owner' || role === 'admin' }
}

/** Owner or admin: may read the agents, "Waiting on you" and the recent
 *  requests, and flip agents' switches. */
function manages(role: PermissionViewerRole | null): boolean {
  return role === 'owner' || role === 'admin'
}

const STATE_LABEL: Record<string, string> = {
  pending: 'Waiting',
  already_done: 'Already on',
  applying: 'Applying',
  approved: 'Allowed',
  denied: 'Denied',
  expired: 'Expired',
  stale: 'Out of date',
  failed: 'Failed',
  cancelled: 'Cancelled',
  closed: 'Closed',
}

export function requestStateLabel(state: string): string {
  return STATE_LABEL[state] ?? state
}

function BetaPill(): React.JSX.Element {
  return (
    <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
      BETA
    </span>
  )
}

/** The global switch and the viewer's role, re-read on a host switch and on
 *  the daemon's permission frames. `on` is null on a server with no
 *  permission cards (no `permissionCards` key). */
type ServerState =
  | { kind: 'loading' }
  | { kind: 'ready'; on: boolean | null }
  | { kind: 'error'; message: string }

function useServerState(): {
  server: ServerState
  role: PermissionViewerRole | null
  nonce: number
  setOn: (on: boolean) => void
} {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [server, setServer] = useState<ServerState>({ kind: 'loading' })
  const [role, setRole] = useState<PermissionViewerRole | null>(null)
  const [nonce, setNonce] = useState(0)

  useEffect(() => onPermissionCardsSync(() => setNonce((n) => n + 1)), [])

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((r) => {
      if (!cancelled) setRole(r)
    })
    fetchPermissionCardsSetting()
      .then((on) => {
        if (!cancelled) setServer({ kind: 'ready', on })
      })
      .catch((err) => {
        if (cancelled) return
        console.error('[permission-cards] settings/get failed', err)
        setServer({ kind: 'error', message: permissionErrorMessage(err) })
      })
    return () => {
      cancelled = true
    }
  }, [hostKey, nonce])

  const setOn = useCallback((on: boolean) => setServer({ kind: 'ready', on }), [])
  return { server, role, nonce, setOn }
}

// ── The Agent tab: context, chip and row ─────────────────────────────

interface AsksContextValue {
  serverOn: boolean | null
  role: PermissionViewerRole | null
  /** This agent's requests that wait on this viewer. */
  pending: PermissionPending[]
  handle: string
  /** Bumps when the daemon says a permission switch moved. */
  nonce: number
}

const AsksContext = createContext<AsksContextValue | null>(null)

/** Wraps the Agent tab: reads the global switch, the viewer's role and (for
 *  an owner/admin while the switch is on) this agent's requests waiting on
 *  them, for the row and the "Asked for this" chips. */
export function PermissionAsksProvider({
  project,
  children,
}: {
  project: { id: string; handle?: string | null }
  children: React.ReactNode
}): React.JSX.Element {
  const { server, role, nonce } = useServerState()
  const serverOn = server.kind === 'ready' ? server.on : null
  const handle = project.handle ?? ''
  const [pending, setPending] = useState<PermissionPending[]>([])
  const live = serverOn === true && manages(role) && handle !== ''

  useEffect(() => {
    if (!live) {
      setPending([])
      return
    }
    let cancelled = false
    const load = (): void => {
      if (!visible()) return
      fetchPermissionPending()
        .then((rows) => {
          if (!cancelled) setPending(rows.filter((p) => p.agent === handle))
        })
        .catch((err) => {
          if (!cancelled) console.warn('[permission-cards] permission/pending failed', err)
        })
    }
    load()
    const id = setInterval(load, POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [live, handle, nonce])

  const value = useMemo<AsksContextValue>(
    () => ({ serverOn, role, pending, handle, nonce }),
    [serverOn, role, pending, handle, nonce],
  )
  return <AsksContext.Provider value={value}>{children}</AsksContext.Provider>
}

/** "Asked for this" beside an Agent tab switch this agent has a card out
 *  for (§8). Click opens the card. Nothing outside PermissionAsksProvider. */
export function AskedChip({ row }: { row: string }): React.JSX.Element | null {
  const ctx = useContext(AsksContext)
  if (!ctx) return null
  const asks = asksForRow(ctx.pending, ctx.handle, row)
  if (asks.length === 0) return null
  const first = asks[0]
  return (
    <button
      type="button"
      data-testid={`permission-asked-${row}`}
      onClick={() => {
        openPermissionCard(first.agent)
      }}
      title={`${first.agent} asked for this (${first.setting}). Open the card in its Thread.`}
      className="ml-1.5 align-middle text-[10px] font-normal px-1.5 py-0.5 border border-[var(--color-status-warn-amber-soft)] text-[var(--color-status-warn-amber-soft)] hover:bg-[var(--color-bg-elevated)] cursor-pointer no-drag"
    >
      Asked for this
    </button>
  )
}

/** The Agent tab's "Can ask for permission cards" (Beta, §2.2): this
 *  agent's own switch, read from and written to the daemon. Shown only
 *  while the server's switch is on, to the owner or an admin. Must sit
 *  inside PermissionAsksProvider. */
export function AgentPermissionCardsRow({ project }: { project: { id: string; name?: string } }): React.JSX.Element | null {
  const ctx = useContext(AsksContext)
  if (!ctx) throw new Error('AgentPermissionCardsRow must sit inside PermissionAsksProvider')
  const { show, canFlip } = agentPermissionRowVisibility(ctx.role, ctx.serverOn)
  const [on, setOn] = useState<boolean | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!show) return
    let cancelled = false
    fetchPermissionAgents(project.id)
      .then(({ agents }) => {
        if (cancelled) return
        const mine = agents.find((a) => a.workspace === project.id)
        if (!mine) {
          setLoadError('The server did not list this agent.')
          return
        }
        setOn(mine.on)
        setLoadError(null)
      })
      .catch((err) => {
        if (cancelled) return
        console.error('[permission-cards] permission/agents failed', err)
        setLoadError(permissionErrorMessage(err))
      })
    return () => {
      cancelled = true
    }
  }, [show, project.id, ctx.nonce])

  if (!show) return null

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || on === null) return
    const before = on
    const next = !on
    setBusy(true)
    setError(null)
    setOn(next)
    try {
      setOn(await setAgentPermissionCards(project.id, next))
    } catch (err) {
      setOn(before)
      setError(permissionErrorMessage(err))
      console.error('[agent-access] permission_cards write failed', err)
    } finally {
      setBusy(false)
    }
  }

  const checked = on === true
  return (
    <div className="py-2 border-t border-[var(--color-border)]">
      <div className="flex items-start gap-3">
        <button
          onClick={() => void toggle()}
          role="switch"
          aria-checked={checked}
          aria-label="Can ask for permission cards"
          disabled={busy || !canFlip || on === null}
          data-settings-id="projects.agents-can-request-permissions"
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            checked ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={
            !canFlip
              ? 'Checking that you are the owner or an admin of this server'
              : checked
                ? 'This agent may ask you for setting changes with a card in its Thread'
                : 'This agent asks the usual way, in its own words'
          }
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              checked ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Can ask for permission cards{' '}
            <span className="text-[10px] font-normal text-[var(--color-text-muted)]">Beta</span>
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            When this agent needs a setting changed (like compute nodes or hosted mail), it can post
            a card in its Thread saying what and why. You or an admin allow or deny it there; nothing
            changes until someone allows it, and every answer is logged in Access Audit. Off: it asks
            the usual way, in its own words.
          </div>
          {loadError ? (
            <div className="text-[10px] text-[var(--color-status-error-soft)] mt-1" data-testid="agent-permission-cards-load-error">
              Couldn&rsquo;t read this switch: {loadError}
            </div>
          ) : null}
          {error ? (
            <div className="text-[10px] text-[var(--color-status-error-soft)] mt-1" data-testid="agent-permission-cards-error">
              {error}
            </div>
          ) : null}
          {ctx.pending.length > 0 ? (
            <div className="text-[10px] text-[var(--color-text-secondary)] mt-1" data-testid="agent-permission-cards-waiting">
              {ctx.pending.length === 1
                ? `1 request from this agent is waiting on you: ${ctx.pending[0].setting}.`
                : `${ctx.pending.length} requests from this agent are waiting on you.`}{' '}
              <button
                type="button"
                className="underline cursor-pointer no-drag text-[var(--color-text-primary)]"
                onClick={() => {
                  openPermissionCard(ctx.pending[0].agent)
                }}
              >
                Open
              </button>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  )
}

// ── The page ──────────────────────────────────────────────────────────

type ListState<T> = { kind: 'idle' } | { kind: 'loading' } | { kind: 'ready'; value: T } | { kind: 'error'; message: string }

export function PermissionCardsSection(): React.JSX.Element {
  const { server, role, nonce, setOn } = useServerState()
  const [globalBusy, setGlobalBusy] = useState(false)
  // The owner's click paints the switch at once; the agents and "Waiting on
  // you" load only once the daemon has stored it (the family is absent while
  // the server switch is off, §2.4).
  const [painted, setPainted] = useState<boolean | null>(null)
  const [globalError, setGlobalError] = useState<string | null>(null)
  const [agents, setAgents] = useState<ListState<{ agents: PermissionAgent[]; recent: PermissionRecent[] }>>({ kind: 'idle' })
  const [pending, setPending] = useState<ListState<PermissionPending[]>>({ kind: 'idle' })
  const [agentErrors, setAgentErrors] = useState<Record<string, string>>({})
  const [busyAgents, setBusyAgents] = useState<Set<string>>(() => new Set())
  const [openError, setOpenError] = useState<string | null>(null)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const serverOn = server.kind === 'ready' ? server.on : null
  const live = serverOn === true && manages(role)

  const load = useCallback((first: boolean): void => {
    if (first) {
      setAgents({ kind: 'loading' })
      setPending({ kind: 'loading' })
    }
    fetchPermissionAgents()
      .then((value) => {
        if (mounted.current) setAgents({ kind: 'ready', value })
      })
      .catch((err) => {
        if (!mounted.current) return
        console.error('[permission-cards] permission/agents failed', err)
        setAgents({ kind: 'error', message: permissionErrorMessage(err) })
      })
    fetchPermissionPending()
      .then((value) => {
        if (mounted.current) setPending({ kind: 'ready', value })
      })
      .catch((err) => {
        if (!mounted.current) return
        console.error('[permission-cards] permission/pending failed', err)
        setPending({ kind: 'error', message: permissionErrorMessage(err) })
      })
  }, [])

  useEffect(() => {
    if (!live) {
      setAgents({ kind: 'idle' })
      setPending({ kind: 'idle' })
      return
    }
    load(true)
    const id = setInterval(() => {
      if (visible()) load(false)
    }, POLL_MS)
    return () => clearInterval(id)
  }, [live, nonce, load])

  const flipGlobal = async (next: boolean): Promise<void> => {
    if (globalBusy || !globalSwitchCanFlip(role) || serverOn === null) return
    setGlobalBusy(true)
    setGlobalError(null)
    setPainted(next)
    try {
      setOn(await setPermissionCardsSetting(next))
    } catch (err) {
      setGlobalError(permissionErrorMessage(err))
      console.error('[permission-cards] settings/update failed', err)
    } finally {
      setPainted(null)
      setGlobalBusy(false)
    }
  }

  const patchAgent = (workspace: string, on: boolean): void => {
    setAgents((s) =>
      s.kind === 'ready'
        ? { kind: 'ready', value: { ...s.value, agents: s.value.agents.map((a) => (a.workspace === workspace ? { ...a, on } : a)) } }
        : s,
    )
  }

  const setBusy = (workspace: string, busy: boolean): void => {
    setBusyAgents((s) => {
      const n = new Set(s)
      if (busy) n.add(workspace)
      else n.delete(workspace)
      return n
    })
  }

  const flipAgent = async (agent: PermissionAgent, next: boolean): Promise<void> => {
    if (busyAgents.has(agent.workspace)) return
    setBusy(agent.workspace, true)
    setAgentErrors(({ [agent.workspace]: _drop, ...rest }) => rest)
    patchAgent(agent.workspace, next)
    try {
      const stored = await setAgentPermissionCards(agent.workspace, next)
      patchAgent(agent.workspace, stored)
    } catch (err) {
      patchAgent(agent.workspace, agent.on)
      setAgentErrors((e) => ({ ...e, [agent.workspace]: permissionErrorMessage(err) }))
      console.error('[agent-access] permission_cards write failed', err)
    } finally {
      setBusy(agent.workspace, false)
    }
  }

  /** "Turn on for all agents": one write per agent that is off, in order;
   *  each paints on now and rolls back alone if the daemon refuses it. */
  const turnOnAll = async (): Promise<void> => {
    if (agents.kind !== 'ready') return
    for (const a of agents.value.agents.filter((x) => !x.on)) {
      await flipAgent(a, true)
    }
  }

  const open = (agent: string): void => {
    setOpenError(null)
    if (!openPermissionCard(agent)) setOpenError(`This server lists no workspace "${agent}".`)
  }

  const canFlipGlobal = globalSwitchCanFlip(role)
  const now = nowSec()

  return (
    <div className="max-w-3xl space-y-6" data-testid="permission-cards-section">
      <div>
        <h2 className="text-lg font-semibold text-[var(--color-text-primary)] flex items-center gap-2">
          Permission Cards <BetaPill />
        </h2>
        <p className="text-xs text-[var(--color-text-muted)] mt-1">
          When an agent hits something only an owner or admin can turn on, it can post a card in its
          Thread that says what it needs and why, instead of you hunting for the switch in Settings.
          You answer on the card; it changes the same setting you would change here, and every answer
          is logged in Access Audit.
        </p>
      </div>

      <SettingsGroup title="Permission cards" badge={<BetaPill />}>
        <div className="flex items-start gap-3 py-2" data-settings-id="permission-cards.switch">
          <Toggle
            checked={(painted ?? serverOn) === true}
            onChange={(v) => void flipGlobal(v)}
            disabled={!canFlipGlobal || globalBusy || serverOn === null}
            aria-label="Permission cards"
          />
          <div className="flex-1 min-w-0">
            <div className="text-xs text-[var(--color-text-primary)]">Permission cards</div>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">
              Lets agents you choose ask for setting changes with a card. Off: nothing changes; agents
              ask the usual way, in their own words. Turning it off closes any cards still waiting.
            </p>
            {server.kind === 'loading' ? (
              <p className="text-[11px] text-[var(--color-text-muted)] mt-1">Reading this server&rsquo;s settings…</p>
            ) : null}
            {server.kind === 'error' ? (
              <p className="text-[11px] text-[var(--color-status-error-soft)] mt-1" data-testid="permission-cards-server-error">
                Couldn&rsquo;t read this server&rsquo;s settings: {server.message}
              </p>
            ) : null}
            {server.kind === 'ready' && server.on === null ? (
              <p className="text-[11px] text-[var(--color-text-muted)] mt-1" data-testid="permission-cards-old-server">
                This server doesn&rsquo;t have permission cards yet. Update it to K2 0.46.3 or later.
              </p>
            ) : null}
            {serverOn !== null && role !== null && !canFlipGlobal ? (
              <p className="text-[11px] text-[var(--color-text-muted)] mt-1" data-testid="permission-cards-owner-only">
                Only the server&rsquo;s owner can turn permission cards on or off.
              </p>
            ) : null}
            {globalError ? (
              <p className="text-[11px] text-[var(--color-status-error-soft)] mt-1" data-testid="permission-cards-switch-error">
                {globalError}
              </p>
            ) : null}
          </div>
        </div>
      </SettingsGroup>

      {serverOn === true && role === 'member' ? (
        <p className="text-xs text-[var(--color-text-muted)]" data-testid="permission-cards-member">
          The server&rsquo;s owner and admins choose which agents can ask, and answer their cards.
        </p>
      ) : null}

      {live ? (
        <>
          <SettingsGroup title="Agents">
            <div data-settings-id="permission-cards.agents">
              <div className="flex items-center justify-between gap-3 py-1">
                <p className="text-[11px] text-[var(--color-text-muted)]">
                  An agent can ask only while its own switch is on too. It&rsquo;s also on each agent&rsquo;s
                  Agent tab.
                </p>
                <button
                  type="button"
                  onClick={() => void turnOnAll()}
                  disabled={agents.kind !== 'ready' || agents.value.agents.every((a) => a.on) || busyAgents.size > 0}
                  className="flex-shrink-0 px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] disabled:opacity-50 cursor-pointer no-drag"
                >
                  Turn on for all agents
                </button>
              </div>
              {agents.kind === 'loading' ? <p className="text-[11px] text-[var(--color-text-muted)] py-1">Loading agents…</p> : null}
              {agents.kind === 'error' ? (
                <p className="text-[11px] text-[var(--color-status-error-soft)] py-1" data-testid="permission-cards-agents-error">
                  Couldn&rsquo;t load the agents: {agents.message}
                </p>
              ) : null}
              {agents.kind === 'ready' && agents.value.agents.length === 0 ? (
                <p className="text-[11px] text-[var(--color-text-muted)] py-1">No agents on this server yet.</p>
              ) : null}
              {agents.kind === 'ready'
                ? agents.value.agents.map((a) => (
                    <div key={a.workspace} className="flex items-start gap-3 py-1.5 border-t border-[var(--color-border)]" data-testid={`permission-agent-${a.handle}`}>
                      <Toggle
                        checked={a.on}
                        onChange={(v) => void flipAgent(a, v)}
                        disabled={busyAgents.has(a.workspace)}
                        aria-label={`${a.name} can ask for permission cards`}
                      />
                      <div className="flex-1 min-w-0">
                        <div className="text-xs text-[var(--color-text-primary)]">
                          {a.name} <span className="text-[10px] text-[var(--color-text-muted)]">{a.handle}</span>
                        </div>
                        {agentErrors[a.workspace] ? (
                          <div className="text-[10px] text-[var(--color-status-error-soft)] mt-0.5">{agentErrors[a.workspace]}</div>
                        ) : null}
                      </div>
                    </div>
                  ))
                : null}
            </div>
          </SettingsGroup>

          <SettingsGroup title="Waiting on you">
            <div data-settings-id="permission-cards.waiting">
              {openError ? <p className="text-[11px] text-[var(--color-status-error-soft)] py-1">{openError}</p> : null}
              {pending.kind === 'loading' ? <p className="text-[11px] text-[var(--color-text-muted)] py-1">Loading…</p> : null}
              {pending.kind === 'error' ? (
                <p className="text-[11px] text-[var(--color-status-error-soft)] py-1" data-testid="permission-cards-pending-error">
                  Couldn&rsquo;t load the cards waiting on you: {pending.message}
                </p>
              ) : null}
              {pending.kind === 'ready' && pending.value.length === 0 ? (
                <p className="text-[11px] text-[var(--color-text-muted)] py-1">Nothing is waiting on you.</p>
              ) : null}
              {pending.kind === 'ready'
                ? pending.value.map((p) => (
                    <div key={p.requestId} className="flex items-start gap-3 py-1.5 border-t border-[var(--color-border)]" data-testid={`permission-pending-${p.requestId}`}>
                      <div className="flex-1 min-w-0">
                        <div className="text-xs text-[var(--color-text-primary)]">
                          {p.agent} asks: {p.setting}
                        </div>
                        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                          {p.whatChanges ? `${p.whatChanges} ` : ''}
                          Asked {formatRelativeTime(p.createdAt, now)}
                          {p.askedFor ? ` while working with ${p.askedFor}` : ''}.
                          {p.proofKind === 'confirm' ? ' Needs a confirm.' : ''}
                        </div>
                      </div>
                      <button
                        type="button"
                        onClick={() => open(p.agent)}
                        className="flex-shrink-0 px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer no-drag"
                      >
                        Open card
                      </button>
                    </div>
                  ))
                : null}
            </div>
          </SettingsGroup>

          <SettingsGroup title="Recent requests">
            <div data-settings-id="permission-cards.recent">
              {agents.kind === 'ready' && agents.value.recent.length === 0 ? (
                <p className="text-[11px] text-[var(--color-text-muted)] py-1">No requests yet.</p>
              ) : null}
              {agents.kind === 'ready'
                ? agents.value.recent.map((r) => (
                    <div key={r.id} className="flex items-start gap-3 py-1.5 border-t border-[var(--color-border)]" data-testid={`permission-recent-${r.id}`}>
                      <div className="flex-1 min-w-0">
                        <div className="text-xs text-[var(--color-text-primary)]">
                          {r.agent || '—'}: {r.setting}
                        </div>
                        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
                          {requestStateLabel(r.state)}
                          {r.answeredBy ? ` by ${r.answeredBy}` : ''} · asked {formatRelativeTime(r.askedAt, now)}
                        </div>
                      </div>
                      {r.state === 'pending' && r.agent ? (
                        <button
                          type="button"
                          onClick={() => open(r.agent)}
                          className="flex-shrink-0 px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer no-drag"
                        >
                          Open card
                        </button>
                      ) : null}
                    </div>
                  ))
                : null}
            </div>
          </SettingsGroup>
        </>
      ) : null}
    </div>
  )
}
