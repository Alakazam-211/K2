// Settings → Access → Permission Cards (Beta), prd-agent-settings-cards-v1
// §2.1, §2.2, §8 (lane D2). Also the Agent tab's "Can ask for permission
// cards" row and its "Asked for this" chip, which ProjectsSection mounts.
//
// Daemon-first: the switches, the agent list, "Waiting on you" and the
// recent requests are all read from the daemon (permission-cards-api.ts);
// every switch is one gesture posted back, painted optimistically and
// rolled back on a refusal. An answer from "Waiting on you" is the card's
// own `thread/answer`. The page re-reads on the daemon's `sync:settings`
// permission frames and, while visible, on a slow poll.
//
// Who sees what (PRD §2.1, §2.2):
//   - the global switch: everyone sees its state; only the owner flips it
//     (the daemon's OWNER_ONLY_KEYS; an admin or member sees it read-only);
//   - the agents, "Waiting on you" and recent requests: only while the
//     global switch is on, and only for the owner or an admin (the per-agent
//     switch's floor). A member sees one line instead;
//   - the Agent tab row: only while the global switch is on, owner/admin.
//
// The page follows the Settings reference pattern: a header with one plain
// line, card groups with label + hint left and the control on the right,
// SettingsLoadState for loading / failed / empty, "Saved" by a switch, and a
// confirm before anything that changes a setting or closes waiting cards.

import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SaveError,
  SaveNote,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
  type SaveState,
  SETTING_LIST_ROW_HOVER,
} from '../controls/SettingControls'
import { BetaTag, Button, Tag, Toggle, type TagTone } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { useConnectHostStore } from '@/stores/connect-host'
import { useSettingsStore } from '@/stores/settings'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { daemonErrorOf } from '@/lib/daemon-error'
import { AccessSheet, ConfirmSheet, DIALOG_MUTED, FailedNote } from './AccessDialogs'
import {
  answerPermissionRequest,
  asksForRow,
  canAllowInPlace,
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
    label: 'Let agents ask with permission cards',
    beta: true,
    description:
      'Let agents ask, with a card in their Thread, for a setting you would otherwise change in Settings. Owner only; on by default',
    keywords: ['permission cards', 'permission', 'cards', 'ask', 'request', 'approve', 'allow', 'deny', 'beta', 'k2 permission'],
  },
  {
    id: 'permission-cards.agents',
    section: 'permission-cards',
    label: 'Agents that can ask',
    description: 'Which agents can ask with a card, and Turn on for all agents',
    keywords: ['permission cards', 'agents', 'turn on for all', 'can ask'],
  },
  {
    id: 'permission-cards.waiting',
    section: 'permission-cards',
    label: 'Requests waiting on you',
    description: 'Allow or deny what agents asked for, and see recent requests',
    keywords: ['waiting on you', 'pending', 'requests', 'permission cards', 'allow', 'deny'],
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

const STATE_TONE: Record<string, TagTone> = {
  pending: 'warn',
  applying: 'warn',
  approved: 'ok',
  already_done: 'ok',
  denied: 'neutral',
  failed: 'danger',
}

/** The server's reason for a refusal, in its own words (hint before code). */
function serverReason(err: unknown): string {
  const info = daemonErrorOf(err)
  if (info) return info.hint || info.code
  return err instanceof Error ? err.message : String(err)
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
  retry: () => void
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
  const retry = useCallback(() => {
    setServer({ kind: 'loading' })
    setNonce((n) => n + 1)
  }, [])
  return { server, role, nonce, setOn, retry }
}

/** A text link inside a hint, with the shared focus ring. */
function TextLink({ onClick, children, testId }: { onClick: () => void; children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <button
      type="button"
      data-k2-focus=""
      data-testid={testId}
      className="relative px-0.5 -mx-0.5 underline text-[var(--color-text-primary)] hover:text-[var(--color-accent-text)] no-drag cursor-pointer"
      onClick={onClick}
    >
      {children}
    </button>
  )
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
 *  for (§8). Click opens the card. Nothing outside PermissionAsksProvider.
 *  Drawn as the shared tag (warn tone) so it reads like the tags around it,
 *  with the shared focus ring. */
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
      data-k2-chamfer="chip"
      data-k2-focus=""
      data-tone="warn"
      data-case="as-written"
      onClick={() => {
        openPermissionCard(first.agent)
      }}
      title={`${first.agent} asked for this (${first.setting}). Opens its card in the agent's Thread.`}
      className="k2-tag type-hint font-medium relative ml-2 align-middle cursor-pointer no-drag hover:brightness-110"
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
  const save = useSaveStatus()
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
    save.reset()
    setOn(next)
    try {
      setOn(await setAgentPermissionCards(project.id, next))
      save.saved()
    } catch (err) {
      setOn(before)
      save.failed(new Error(serverReason(err)))
      console.error('[agent-access] permission_cards write failed', err)
    } finally {
      setBusy(false)
    }
  }

  const checked = on === true
  return (
    <SettingSwitchRow
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle
            checked={checked}
            onChange={() => void toggle()}
            aria-label="Let this agent ask for permission"
            disabled={busy || !canFlip || on === null}
            data-settings-id="projects.agents-can-request-permissions"
            title={
              !canFlip
                ? 'Checking that you are the owner or an admin of this server'
                : checked
                  ? 'This agent may ask you for setting changes with a card in its Thread'
                  : 'This agent asks the usual way, in its own words'
            }
          />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>
        Let this agent ask for permission
        <BetaTag className="ml-2" />
      </div>
      <div className={SETTING_ROW_HINT}>
        It can post a card in its Thread when it needs a setting changed, saying what and why.
        Nothing changes until you or an admin allow it.
      </div>
      {loadError ? (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]" data-testid="agent-permission-cards-load-error">
          Couldn&rsquo;t read this switch, so it&rsquo;s off for now. The server said: {loadError.replace(/[.\s]+$/, '')}.
        </p>
      ) : null}
      <div data-testid={typeof save.state === 'object' ? 'agent-permission-cards-error' : undefined}>
        <SaveError state={save.state} />
      </div>
      {ctx.pending.length > 0 ? (
        <div className={SETTING_ROW_HINT} data-testid="agent-permission-cards-waiting">
          {ctx.pending.length === 1
            ? `1 request from this agent is waiting on you: ${ctx.pending[0].setting}.`
            : `${ctx.pending.length} requests from this agent are waiting on you.`}{' '}
          <TextLink onClick={() => void openPermissionCard(ctx.pending[0].agent)}>Open the card</TextLink>
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}

// ── The page ──────────────────────────────────────────────────────────

type ListState<T> = { kind: 'idle' } | { kind: 'loading' } | { kind: 'ready'; value: T } | { kind: 'error'; message: string }

type AgentResult = { kind: 'saved'; n: number } | { kind: 'error'; message: string; n: number }

type Notice = { tone: 'ok'; text: string } | { tone: 'error'; text: string; detail: string }

export function PermissionCardsSection(): React.JSX.Element {
  const { server, role, nonce, setOn, retry } = useServerState()
  const [globalBusy, setGlobalBusy] = useState(false)
  // The owner's click paints the switch at once; the agents and "Waiting on
  // you" load only once the daemon has stored it (the family is absent while
  // the server switch is off, §2.4).
  const [painted, setPainted] = useState<boolean | null>(null)
  const globalSave = useSaveStatus()
  const [confirmOff, setConfirmOff] = useState(false)
  const [agents, setAgents] = useState<ListState<{ agents: PermissionAgent[]; recent: PermissionRecent[] }>>({ kind: 'idle' })
  const [pending, setPending] = useState<ListState<PermissionPending[]>>({ kind: 'idle' })
  const [agentResults, setAgentResults] = useState<Record<string, AgentResult>>({})
  const [busyAgents, setBusyAgents] = useState<Set<string>>(() => new Set())
  const [answering, setAnswering] = useState<string | null>(null)
  const [allowing, setAllowing] = useState<PermissionPending | null>(null)
  const [notice, setNotice] = useState<Notice | null>(null)
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const serverOn = server.kind === 'ready' ? server.on : null
  const live = serverOn === true && manages(role)

  const loadAgents = useCallback((first: boolean): void => {
    if (first) setAgents({ kind: 'loading' })
    fetchPermissionAgents()
      .then((value) => {
        if (mounted.current) setAgents({ kind: 'ready', value })
      })
      .catch((err) => {
        if (!mounted.current) return
        console.error('[permission-cards] permission/agents failed', err)
        setAgents({ kind: 'error', message: permissionErrorMessage(err) })
      })
  }, [])

  const loadPending = useCallback((first: boolean): void => {
    if (first) setPending({ kind: 'loading' })
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

  const load = useCallback(
    (first: boolean): void => {
      loadAgents(first)
      loadPending(first)
    },
    [loadAgents, loadPending],
  )

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

  const writeGlobal = async (next: boolean): Promise<void> => {
    setGlobalBusy(true)
    globalSave.reset()
    setPainted(next)
    try {
      setOn(await setPermissionCardsSetting(next))
      globalSave.saved()
    } catch (err) {
      globalSave.failed(new Error(serverReason(err)))
      console.error('[permission-cards] settings/update failed', err)
    } finally {
      setPainted(null)
      setGlobalBusy(false)
    }
  }

  const waitingCount = pending.kind === 'ready' ? pending.value.length : 0

  const flipGlobal = (next: boolean): void => {
    if (globalBusy || !globalSwitchCanFlip(role) || serverOn === null) return
    // Turning it off closes the cards still waiting: ask first.
    if (!next && waitingCount > 0) {
      setConfirmOff(true)
      return
    }
    void writeGlobal(next)
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
    patchAgent(agent.workspace, next)
    try {
      const stored = await setAgentPermissionCards(agent.workspace, next)
      patchAgent(agent.workspace, stored)
      setAgentResults((r) => ({ ...r, [agent.workspace]: { kind: 'saved', n: (r[agent.workspace]?.n ?? 0) + 1 } }))
    } catch (err) {
      patchAgent(agent.workspace, agent.on)
      setAgentResults((r) => ({ ...r, [agent.workspace]: { kind: 'error', message: serverReason(err), n: (r[agent.workspace]?.n ?? 0) + 1 } }))
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

  const agentName = (handle: string): string => {
    if (!handle) return 'An agent'
    const a = agents.kind === 'ready' ? agents.value.agents.find((x) => x.handle === handle) : undefined
    return a?.name ?? handle
  }

  const openCard = (agent: string): void => {
    if (!openPermissionCard(agent)) {
      setNotice({ tone: 'error', text: `Couldn’t open ${agentName(agent)}’s card`, detail: `this server lists no workspace "${agent}"` })
    }
  }

  /** Allow (from its dialog) or Deny one request: the card's own answer. */
  const answer = async (p: PermissionPending, verdict: 'Allow' | 'Deny'): Promise<void> => {
    setAnswering(p.requestId)
    setNotice(null)
    const who = agentName(p.agent)
    try {
      await answerPermissionRequest(p, verdict)
      setPending((s) => (s.kind === 'ready' ? { kind: 'ready', value: s.value.filter((x) => x.requestId !== p.requestId) } : s))
      setAllowing(null)
      setNotice({
        tone: 'ok',
        text:
          verdict === 'Allow'
            ? `Allowed ${who}’s request: ${p.setting}. It’s in Recent requests below.`
            : `Denied ${who}’s request: ${p.setting}. It can ask again later.`,
      })
      load(false)
    } catch (err) {
      console.error('[permission-cards] thread/answer failed', err)
      const gone = daemonErrorOf(err)?.code === 'request_not_pending'
      if (gone) load(false)
      if (verdict === 'Allow') throw err
      setNotice({
        tone: 'error',
        text: gone ? `${who}’s request was already answered or closed` : `Couldn’t deny ${who}’s request`,
        detail: serverReason(err),
      })
    } finally {
      if (mounted.current) setAnswering(null)
    }
  }

  const canFlipGlobal = globalSwitchCanFlip(role)
  const now = nowSec()
  const shownOn = (painted ?? serverOn) === true

  const header = (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
        Permission Cards <BetaTag />
      </h2>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
        When an agent needs a setting that only an owner or admin can change, it can ask with a card in its Thread
        that says what it wants and why. Nothing changes until someone allows it.
      </p>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1">
        Every answer is logged in{' '}
        <TextLink onClick={() => useSettingsStore.getState().setSection('access-audit')}>Logs → Access Audit</TextLink>.
      </p>
    </header>
  )

  let body: React.ReactNode
  if (server.kind === 'loading') {
    body = (
      <SettingsLoadState kind="loading" title="Reading this server’s settings…" testId="permission-cards-loading">
        Checking whether permission cards are on.
      </SettingsLoadState>
    )
  } else if (server.kind === 'error') {
    body = (
      <SettingsLoadState
        kind="error"
        title="Couldn’t read this server’s settings"
        detail={server.message}
        action={<Button onClick={retry}>Try again</Button>}
        testId="permission-cards-server-error"
      >
        Nothing has changed. Try again in a moment.
      </SettingsLoadState>
    )
  } else if (server.on === null) {
    body = (
      <SettingsLoadState kind="empty" title="This server doesn’t have permission cards yet" testId="permission-cards-old-server">
        Update K2 on this server to version 0.46.3 or later to use them. Until then, agents ask in their own words.
      </SettingsLoadState>
    )
  } else {
    body = (
      <>
        <SettingsGroup title="On this server" card>
          <SettingSwitchRow
            settingId="permission-cards.switch"
            control={
              <>
                <SaveNote state={globalSave.state} />
                <Toggle
                  checked={shownOn}
                  onChange={(v) => flipGlobal(v)}
                  disabled={!canFlipGlobal || globalBusy}
                  aria-label="Let agents ask with permission cards"
                  title={canFlipGlobal ? undefined : 'Only this server’s owner can turn this on or off'}
                />
              </>
            }
          >
            <div className={SETTING_ROW_LABEL}>Let agents ask with permission cards</div>
            <div className={SETTING_ROW_HINT}>
              {shownOn
                ? manages(role)
                  ? 'The agents you choose below can post a card when they need a setting changed. Turning this off closes any cards still waiting.'
                  : 'Agents the owner or an admin chooses can post a card when they need a setting changed.'
                : canFlipGlobal
                  ? 'Off: agents ask in their own words, and no cards are posted. Turn it on, then choose which agents can ask.'
                  : 'Off: agents ask in their own words, and no cards are posted.'}
            </div>
            {role !== null && !canFlipGlobal ? (
              <div className={SETTING_ROW_HINT} data-testid="permission-cards-owner-only">
                Only this server&rsquo;s owner can turn this on or off.
              </div>
            ) : null}
            <div data-testid={typeof globalSave.state === 'object' ? 'permission-cards-switch-error' : undefined}>
              <SaveError state={globalSave.state} />
            </div>
          </SettingSwitchRow>
        </SettingsGroup>

        {server.on && role === 'member' ? (
          <SettingsLoadState kind="empty" title="The owner and admins answer these cards" testId="permission-cards-member">
            They choose which agents can ask, and allow or deny each request. Ask one of them if an agent is waiting on an
            answer.
          </SettingsLoadState>
        ) : null}

        {!server.on && manages(role) ? (
          <SettingsGroup
            title="Agents that can ask"
            card
            state={
              <SettingsLoadState kind="empty" title="Permission cards are off" testId="permission-cards-off">
                {canFlipGlobal
                  ? 'Turn them on above, then choose which agents can ask.'
                  : 'When the owner turns them on, you can choose which agents can ask.'}
              </SettingsLoadState>
            }
          />
        ) : null}

        {live ? (
          <>
            <SettingsGroup
              title="Waiting on you"
              hint="Requests you can answer. The card in the agent’s Thread also says why it’s asking."
              settingId="permission-cards.waiting"
              card
              state={
                pending.kind === 'loading' || pending.kind === 'idle' ? (
                  <SettingsLoadState kind="loading" title="Loading requests…">
                    Reading what agents have asked for.
                  </SettingsLoadState>
                ) : pending.kind === 'error' ? (
                  <SettingsLoadState
                    kind="error"
                    title="Couldn’t load the requests waiting on you"
                    detail={pending.message}
                    action={<Button onClick={() => loadPending(true)}>Try again</Button>}
                    testId="permission-cards-pending-error"
                  >
                    Nothing has been answered. Try again in a moment.
                  </SettingsLoadState>
                ) : pending.value.length === 0 && !notice ? (
                  <SettingsLoadState kind="empty" title="Nothing is waiting on you" testId="permission-cards-pending-empty">
                    When an agent asks, its request shows here and as a card in its Thread.
                  </SettingsLoadState>
                ) : undefined
              }
            >
              {notice ? <NoticeRow notice={notice} onDismiss={() => setNotice(null)} /> : null}
              {pending.kind === 'ready'
                ? pending.value.map((p) => (
                    <PendingRow
                      key={p.requestId}
                      request={p}
                      name={agentName(p.agent)}
                      now={now}
                      busy={answering === p.requestId}
                      onAllow={() => setAllowing(p)}
                      onDeny={() => void answer(p, 'Deny')}
                      onOpen={() => openCard(p.agent)}
                    />
                  ))
                : null}
              {pending.kind === 'ready' && pending.value.length === 0 && notice ? (
                <p className={`px-4 py-3 ${SETTING_ROW_HINT}`}>Nothing else is waiting on you.</p>
              ) : null}
            </SettingsGroup>

            <SettingsGroup
              title="Agents that can ask"
              hint="An agent can ask only while its own switch is on. The same switch is on each agent’s Agent tab."
              settingId="permission-cards.agents"
              action={
                agents.kind === 'ready' && agents.value.agents.length > 0 ? (
                  <Button
                    onClick={() => void turnOnAll()}
                    disabled={agents.value.agents.every((a) => a.on) || busyAgents.size > 0}
                    title={agents.value.agents.every((a) => a.on) ? 'Every agent can already ask' : undefined}
                  >
                    Turn on for all agents
                  </Button>
                ) : undefined
              }
              card
              state={
                agents.kind === 'loading' || agents.kind === 'idle' ? (
                  <SettingsLoadState kind="loading" title="Loading agents…">
                    Reading which agents can ask.
                  </SettingsLoadState>
                ) : agents.kind === 'error' ? (
                  <SettingsLoadState
                    kind="error"
                    title="Couldn’t load the agents"
                    detail={agents.message}
                    action={<Button onClick={() => loadAgents(true)}>Try again</Button>}
                    testId="permission-cards-agents-error"
                  >
                    Nothing has changed. Try again in a moment.
                  </SettingsLoadState>
                ) : agents.value.agents.length === 0 ? (
                  <SettingsLoadState kind="empty" title="No agents on this server yet">
                    Each workspace&rsquo;s agent shows here once you add one in Workspaces / Agents.
                  </SettingsLoadState>
                ) : undefined
              }
            >
              {agents.kind === 'ready'
                ? agents.value.agents.map((a) => (
                    <AgentRow
                      key={a.workspace}
                      agent={a}
                      busy={busyAgents.has(a.workspace)}
                      result={agentResults[a.workspace]}
                      onFlip={(v) => void flipAgent(a, v)}
                    />
                  ))
                : null}
            </SettingsGroup>

            {agents.kind !== 'error' ? (
              <SettingsGroup
                title="Recent requests"
                hint="What agents asked for lately, and how each was answered."
                settingId="permission-cards.recent"
                card
                state={
                  agents.kind !== 'ready' ? (
                    <SettingsLoadState kind="loading" title="Loading recent requests…" />
                  ) : agents.value.recent.length === 0 ? (
                    <SettingsLoadState kind="empty" title="No requests yet">
                      Answered requests show here, with who answered and when.
                    </SettingsLoadState>
                  ) : undefined
                }
              >
                {agents.kind === 'ready'
                  ? agents.value.recent.map((r) => <RecentRow key={r.id} request={r} name={agentName(r.agent)} now={now} />)
                  : null}
              </SettingsGroup>
            ) : null}
          </>
        ) : null}
      </>
    )
  }

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6" data-testid="permission-cards-section">
      {header}
      {body}

      {allowing ? (
        <AllowDialog
          request={allowing}
          name={agentName(allowing.agent)}
          busy={answering === allowing.requestId}
          onAllow={() => answer(allowing, 'Allow')}
          onOpen={() => {
            const agent = allowing.agent
            setAllowing(null)
            openCard(agent)
          }}
          onClose={() => setAllowing(null)}
        />
      ) : null}
      {confirmOff ? (
        <ConfirmSheet
          title="Turn off permission cards?"
          message={`${
            waitingCount === 1 ? 'The request waiting on you closes' : `The ${waitingCount} requests waiting on you close`
          }, and agents go back to asking in their own words. You can turn cards on again at any time.`}
          confirmLabel="Turn off"
          destructive
          onCancel={() => setConfirmOff(false)}
          onConfirm={() => {
            setConfirmOff(false)
            void writeGlobal(false)
          }}
          testId="permission-cards-off-confirm"
        />
      ) : null}
    </div>
  )
}

/** One agent's switch: its name and handle, "Saved" or why it failed. */
function AgentRow({
  agent: a,
  busy,
  result,
  onFlip,
}: {
  agent: PermissionAgent
  busy: boolean
  result: AgentResult | undefined
  onFlip: (next: boolean) => void
}): React.JSX.Element {
  const save = useSaveStatus()
  const saveRef = useRef(save)
  saveRef.current = save
  useEffect(() => {
    if (!result) return
    if (result.kind === 'saved') saveRef.current.saved()
    else saveRef.current.failed(new Error(result.message))
  }, [result])
  const state: SaveState = busy ? 'idle' : save.state
  return (
    <SettingSwitchRow
      testId={`permission-agent-${a.handle}`}
      control={
        <>
          <SaveNote state={state} />
          <Toggle checked={a.on} onChange={onFlip} disabled={busy} aria-label={`${a.name} can ask for permission cards`} />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>{a.name}</div>
      <div className={SETTING_ROW_HINT}>
        <span className="font-mono">{a.handle}</span> · {a.on ? 'Can ask with a card' : 'Asks in its own words'}
      </div>
      <SaveError state={state} />
    </SettingSwitchRow>
  )
}

/** One request waiting on this viewer: what changes, who asked and when, a
 *  link to its card, then Deny (quiet) and Allow… (asks first). */
function PendingRow({
  request: p,
  name,
  now,
  busy,
  onAllow,
  onDeny,
  onOpen,
}: {
  request: PermissionPending
  name: string
  now: number
  busy: boolean
  onAllow: () => void
  onDeny: () => void
  onOpen: () => void
}): React.JSX.Element {
  const inPlace = canAllowInPlace(p)
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`permission-pending-${p.requestId}`}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className={SETTING_ROW_LABEL}>{p.whatChanges || p.setting}</div>
          <div className={SETTING_ROW_HINT}>
            {name} asked {formatRelativeTime(p.createdAt, now)}
            {p.askedFor ? `, while working with ${p.askedFor}` : ''}. Setting: {p.setting}.{' '}
            <TextLink onClick={onOpen}>{inPlace ? 'Read why on its card' : 'Open its card'}</TextLink>
          </div>
          {!inPlace ? (
            <div className={SETTING_ROW_HINT}>Allowing this needs a confirm step, so it&rsquo;s answered on the card.</div>
          ) : null}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <Button variant="quiet" onClick={onDeny} disabled={busy} aria-label={`Deny ${name}’s request`}>
            {busy ? 'Denying…' : 'Deny'}
          </Button>
          {inPlace ? (
            <Button variant="accent" onClick={onAllow} disabled={busy} aria-label={`Allow ${name}’s request…`}>
              Allow…
            </Button>
          ) : (
            <Button variant="accent" onClick={onOpen} disabled={busy}>
              Open card
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}

/** One recent request: what it asked for, who asked and when, and the
 *  answer (who gave it, and when). */
function RecentRow({ request: r, name, now }: { request: PermissionRecent; name: string; now: number }): React.JSX.Element {
  const label = requestStateLabel(r.state)
  const answered = r.answeredBy ? `${label} by ${r.answeredBy}` : label
  return (
    <div className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`permission-recent-${r.id}`}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className={SETTING_ROW_LABEL}>{r.setting}</div>
          <div className={SETTING_ROW_HINT}>
            {name} asked {formatRelativeTime(r.askedAt, now)}
            {r.answeredAt ? ` · answered ${formatRelativeTime(r.answeredAt, now)}` : ''}
          </div>
        </div>
        <Tag tone={STATE_TONE[r.state] ?? 'muted'} data-case="as-written" className="mt-0.5">
          {answered}
        </Tag>
      </div>
    </div>
  )
}

/** What the last answer did, at the top of "Waiting on you". */
function NoticeRow({ notice, onDismiss }: { notice: Notice; onDismiss: () => void }): React.JSX.Element {
  return (
    <div className="px-4 py-2.5 flex items-start justify-between gap-4" data-testid="permission-cards-notice">
      {notice.tone === 'ok' ? (
        <p role="status" className="flex items-start gap-1.5 type-hint text-[var(--color-status-ok-soft)] leading-relaxed">
          <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 mt-0.5 flex-shrink-0">
            <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
          </svg>
          {notice.text}
        </p>
      ) : (
        <div role="alert" className="min-w-0">
          <p className="type-row text-[var(--color-status-error-soft)]">{notice.text}</p>
          <p className={SETTING_ROW_HINT}>
            Nothing changed. Try again, or open the card. The server said: {notice.detail.trim().replace(/[.\s]+$/, '')}.
          </p>
        </div>
      )}
      <Button variant="quiet" className="flex-shrink-0" onClick={onDismiss}>
        Dismiss
      </Button>
    </div>
  )
}

/** Allow one request: what it changes, where to read why, then Allow. A
 *  refusal stays in the dialog with the server's reason. */
function AllowDialog({
  request: p,
  name,
  busy,
  onAllow,
  onOpen,
  onClose,
}: {
  request: PermissionPending
  name: string
  busy: boolean
  onAllow: () => Promise<void>
  onOpen: () => void
  onClose: () => void
}): React.JSX.Element {
  const [failed, setFailed] = useState<{ gone: boolean; detail: string } | null>(null)
  const go = async (): Promise<void> => {
    setFailed(null)
    try {
      await onAllow()
    } catch (err) {
      setFailed({ gone: daemonErrorOf(err)?.code === 'request_not_pending', detail: serverReason(err) })
    }
  }
  return (
    <AccessSheet title={`Allow ${name}’s request?`} onClose={onClose} busy={busy} testId="permission-allow-dialog">
      <div className="space-y-1">
        <p className={SETTING_ROW_LABEL}>{p.whatChanges || p.setting}</p>
        <p className={DIALOG_MUTED}>
          Setting: {p.setting}. It changes as soon as you allow it, and your answer is logged in Access Audit. Why the
          agent is asking is on its card.
        </p>
      </div>
      {failed ? (
        <FailedNote
          what={failed.gone ? 'This request was already answered or closed' : 'Couldn’t allow this request'}
          next={failed.gone ? 'Nothing changed here. Close this and check Recent requests.' : 'Nothing changed. Try again, or open the card.'}
          detail={failed.detail}
        />
      ) : null}
      <div className="flex items-center justify-between gap-2">
        <Button variant="ghost" size="md" onClick={onOpen} disabled={busy}>
          Open card
        </Button>
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            {failed?.gone ? 'Close' : 'Cancel'}
          </Button>
          {failed?.gone ? null : (
            <Button variant="accent" size="md" onClick={() => void go()} disabled={busy}>
              {busy ? 'Allowing…' : 'Allow'}
            </Button>
          )}
        </div>
      </div>
    </AccessSheet>
  )
}
