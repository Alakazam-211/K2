// People and channels on Home: the pickers (prd-home-people-channels-v1
// §6, §7; slice S6). They open from the Add picker on Home's footer.
//
//   - Add a person: one list with a section per server you're signed in
//     to, each asked with that server's own login (`/cli/home-chat/people`).
//     Only people: owner, admin, member and Voice & Chat only logins you
//     share the server with. Never App users, disabled logins or agents (the
//     daemon lists none of them; the client drops anything else too). A
//     person whose chat is still being set up shows "Setting up…".
//     Adding writes a `person` row; the DM is opened on the first open.
//     A match on another server asks "Same person as …? Keep one row." (§6.3).
//   - New channel: a server, a name, Invite-only (the default) or Open, and
//     people. The daemon makes it over mgmt with a random slug; the row goes
//     on the Home once Sew answers with the channel's id.
//   - Add a channel: a server's text channels you can see (browse), joined
//     on open when it is open and you aren't in it yet.
//
// A server where you are Voice & Chat only is listed here, for people and
// channels only (HP-14). A Mac server, or one with chat off, says "No chat
// on this server".

import { useEffect, useMemo, useRef, useState, type JSX } from 'react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useHomesStore, type Home } from '@/stores/homes'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { LOCAL_HOME_HOST, activeHomeHostKey, homeHostKey } from '@/lib/home-address'
import { channelAddress, homeChatTarget, personAddress } from '@/lib/home-chat/address'
import {
  createHomeChatChannel,
  fetchHomeChatChannels,
  fetchHomeChatMembers,
  type HomeChatChannel,
  type HomeChatPerson,
} from '@/lib/home-chat/api'
import { matchesElsewhere, pickablePerson } from '@/lib/home-chat/people'
import { daemonErrorText } from '@/lib/daemon-error'
import { refreshHomeChatHost, refreshHomeChatPeople, scopeForChatHost, useHomeChatStore, type HomeChatHostState } from '@/stores/home-chat'
import { SearchableAgentList, tokenAndMatch, type AgentListRow, type AgentListSection } from '@/components/ui/SearchableAgentList'
import { AgentListStatus, agentListStatusClass } from '@/components/ui/AgentListStatus'
import { serverLabelFor } from './serverLabel'

const itemClass =
  'no-drag w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px] transition-colors cursor-pointer text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)]'
const headClass = 'px-3 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]'
const listClass = 'flex-1 min-h-0 overflow-y-auto py-1'

/** "No chat on this server" and why, from a status read. */
export function noChatLine(reason: string | null): string {
  if (reason === 'mac') return 'No chat on this server (chat runs on Linux servers).'
  if (reason === 'space_pending') return 'Chat is still being set up on this server.'
  return 'No chat on this server.'
}

export interface ChatServer {
  key: string
  label: string
}

/** The servers to ask: the connected one first, This computer when the
 *  window is on another, then every saved server by name. Voice & Chat only
 *  servers stay (people and channels only, HP-14). */
export function chatServers(connectedKey: string, connectedLabel: string, hosts: readonly ConnectHost[]): ChatServer[] {
  const out: ChatServer[] = [{ key: connectedKey, label: connectedLabel }]
  if (connectedKey !== LOCAL_HOME_HOST) out.push({ key: LOCAL_HOME_HOST, label: 'This computer' })
  const seen = new Set(out.map((s) => s.key))
  const sorted = [...hosts].sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }))
  for (const h of sorted) {
    const key = homeHostKey(h)
    if (seen.has(key)) continue
    seen.add(key)
    out.push({ key, label: h.label })
  }
  return out
}

function useChatServers(): ChatServer[] {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  return useMemo(() => {
    const key = activeHomeHostKey(activeHost)
    const label = activeHost === 'local' ? 'This computer' : activeHost.label
    return chatServers(key, label, hosts)
  }, [activeHost, hosts])
}

/** Read every server's chat status and (when on) its people. */
function useServersRead(servers: readonly ChatServer[], people: boolean): void {
  const keys = servers.map((s) => s.key).join('\n')
  useEffect(() => {
    for (const key of keys ? keys.split('\n') : []) {
      void refreshHomeChatHost(key).then(() => {
        const st = useHomeChatStore.getState().byHost[key]
        if (people && st?.status?.on) void refreshHomeChatPeople(key, true)
      })
    }
  }, [keys, people])
}

function BackHead({ label, onBack }: { label: string; onBack: () => void }): JSX.Element {
  return (
    <button type="button" className={`${itemClass} ${headClass} normal-case flex-shrink-0`} onClick={onBack}>
      ‹ {label}
    </button>
  )
}

function glyph(text: string): AgentListRow['avatar'] {
  return {
    kind: 'node',
    node: (
      <span aria-hidden="true" className="inline-flex h-5 w-5 items-center justify-center text-[11px] text-[var(--color-text-muted)]">
        {text}
      </span>
    ),
  }
}

/** A server section's status line while it has no list to show (none: the
 *  list shows). */
function sectionStatus(st: HomeChatHostState | undefined, needPeople: boolean): JSX.Element | undefined {
  if (!st || (!st.status && !st.statusError)) return <AgentListStatus text="Loading…" />
  if (st.statusError) return <AgentListStatus marker="offline" text={noChatLine(null)} />
  if (st.status && !st.status.on) return <AgentListStatus marker="no-access" text={noChatLine(st.status.reason)} />
  if (needPeople && !st.people) return <AgentListStatus text="Loading people…" />
  return undefined
}

// ── Add a person ──────────────────────────────────────────────────────

/** The person rows of one server (people only: never agents or App users). */
function listedPeople(st: HomeChatHostState | undefined): HomeChatPerson[] {
  return (st?.people?.people ?? []).filter((p) => /^(k2:owner|k2:connect:)/.test(p.ext))
}

export function AddPersonPanel({ home, onBack, onAdded }: { home: Home; onBack: () => void; onAdded?: (address: string) => void }): JSX.Element {
  const servers = useChatServers()
  useServersRead(servers, true)
  const [query, setQuery] = useState('')
  const byHost = useHomeChatStore((s) => s.byHost)
  const hosts = useConnectHostStore((s) => s.hosts)

  const sections = useMemo((): AgentListSection[] => {
    const onHome = new Set(home.rows.map((r) => r.address))
    return servers.map((server) => {
      const st = byHost[server.key]
      const status = sectionStatus(st, true)
      const rows: AgentListRow[] = status
        ? []
        : listedPeople(st).map((p) => {
            const address = personAddress(p.ext, server.key) ?? ''
            const taken = onHome.has(address)
            const ready = pickablePerson(p)
            return {
              value: `${server.key}\n${p.ext}`,
              label: p.displayName,
              detail: ready ? (p.role ? `${p.username} · ${p.role}` : p.username) : 'Setting up…',
              avatar: glyph(p.displayName.trim()[0]?.toUpperCase() ?? '?'),
              state: taken ? 'checked' : ready ? 'pickable' : 'disabled',
              title: taken
                ? `Already on ${home.name}`
                : ready
                  ? `Add ${p.displayName} to ${home.name}`
                  : `${p.displayName}'s chat is still being set up`,
              keywords: [p.username],
            }
          })
      return {
        key: server.key,
        label: server.label,
        count: status ? null : undefined,
        status: status ?? (rows.length === 0 ? <p className={agentListStatusClass}>Nobody else on this server</p> : undefined),
        rows,
      }
    })
  }, [servers, byHost, home])

  const onPick = async (value: string): Promise<void> => {
    const [hostKey, ext] = value.split('\n')
    const person = listedPeople(useHomeChatStore.getState().byHost[hostKey]).find((p) => p.ext === ext)
    const address = personAddress(ext, hostKey)
    if (!person || !address || !pickablePerson(person)) return
    // §6.3: a match among this Home's people on another server asks first.
    const rowsByHost: Record<string, HomeChatPerson[]> = {}
    for (const r of home.rows) {
      const t = homeChatTarget(r)
      if (t?.kind !== 'person' || t.hostKey === hostKey) continue
      const p = useHomeChatStore.getState().byHost[t.hostKey]?.people?.people.find((x) => x.ext === t.ext)
      if (p) rowsByHost[t.hostKey] = [...(rowsByHost[t.hostKey] ?? []), p]
    }
    const match = matchesElsewhere(person, hostKey, rowsByHost)[0]
    if (match) {
      const same = await useConfirmDialogStore.getState().confirm({
        title: `Same person as ${match.person.displayName} on ${serverLabelFor(match.hostKey, hosts)}?`,
        message: `Keep one row: ${match.person.displayName}'s row stays, and its menu's “Also on” switches servers. Cancel adds a second row, for two different people.`,
        confirmLabel: 'Keep one row',
      })
      if (same) return
    }
    if (useHomesStore.getState().addRow(home.id, { kind: 'person', address, workspaceId: null, label: person.displayName })) {
      onAdded?.(address)
    }
  }

  return (
    <>
      <BackHead label="Add a person" onBack={onBack} />
      <SearchableAgentList
        sections={sections}
        query={query}
        onQuery={setQuery}
        onPick={(v) => void onPick(v)}
        matches={tokenAndMatch}
        placeholder="Search people..."
        emptyText="Nobody matches"
        ariaLabel="People on your servers"
        listClassName={listClass}
        autoFocus
      />
    </>
  )
}

// ── Add an existing channel ───────────────────────────────────────────

type ChannelListing = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ok'; channels: HomeChatChannel[] }

export function AddChannelPanel({ home, onBack, onAdded }: { home: Home; onBack: () => void; onAdded?: (address: string) => void }): JSX.Element {
  const servers = useChatServers()
  useServersRead(servers, false)
  const [query, setQuery] = useState('')
  const byHost = useHomeChatStore((s) => s.byHost)
  const [listings, setListings] = useState<Record<string, ChannelListing>>({})
  const asked = useRef(new Set<string>())
  const alive = useRef(true)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  // Each server with chat on lists its channels once (browse, HP-27).
  useEffect(() => {
    for (const s of servers) {
      if (!byHost[s.key]?.status?.on || asked.current.has(s.key)) continue
      asked.current.add(s.key)
      fetchHomeChatChannels(scopeForChatHost(s.key)).then(
        (channels) => alive.current && setListings((cur) => ({ ...cur, [s.key]: { kind: 'ok', channels } })),
        (e: unknown) =>
          alive.current && setListings((cur) => ({ ...cur, [s.key]: { kind: 'error', message: daemonErrorText(e) || String(e) } })),
      )
    }
  }, [servers, byHost])

  const sections = useMemo((): AgentListSection[] => {
    const onHome = new Set(home.rows.map((r) => r.address))
    return servers.map((server) => {
      const status = sectionStatus(byHost[server.key], false)
      const listing = listings[server.key] ?? { kind: 'loading' }
      const channels = !status && listing.kind === 'ok' ? listing.channels.filter((c) => !c.archived) : []
      const rows: AgentListRow[] = channels.map((c) => {
        const address = channelAddress(c.conv, server.key) ?? ''
        const taken = onHome.has(address)
        return {
          value: `${server.key}\n${c.conv}`,
          label: c.name,
          detail: c.open ? 'open' : 'invite-only',
          avatar: glyph(c.open ? '#' : '🔒'),
          state: taken ? 'checked' : 'pickable',
          title: taken ? `Already on ${home.name}` : `Add #${c.name} to ${home.name}`,
        }
      })
      const own =
        status ??
        (listing.kind === 'loading' ? (
          <AgentListStatus text="Loading channels…" />
        ) : listing.kind === 'error' ? (
          <AgentListStatus marker="error" tone="error" text={`Couldn’t list channels: ${listing.message}`} />
        ) : rows.length === 0 ? (
          <p className={agentListStatusClass}>No channels yet: make one with New channel</p>
        ) : undefined)
      return { key: server.key, label: server.label, count: own && rows.length === 0 ? null : undefined, status: own, rows }
    })
  }, [servers, byHost, listings, home])

  const onPick = (value: string): void => {
    const [hostKey, conv] = value.split('\n')
    const listing = listings[hostKey]
    const c = listing?.kind === 'ok' ? listing.channels.find((x) => x.conv === conv) : undefined
    const address = channelAddress(conv, hostKey)
    if (!c || !address) return
    // Joined when it opens (the daemon's `view`, HP-27).
    if (useHomesStore.getState().addRow(home.id, { kind: 'channel', address, workspaceId: null, label: c.name })) onAdded?.(address)
  }
  return (
    <>
      <BackHead label="Add a channel" onBack={onBack} />
      <SearchableAgentList
        sections={sections}
        query={query}
        onQuery={setQuery}
        onPick={onPick}
        matches={tokenAndMatch}
        placeholder="Search channels..."
        emptyText="No channels match"
        ariaLabel="Channels on your servers"
        listClassName={listClass}
        autoFocus
      />
    </>
  )
}

// ── New channel ───────────────────────────────────────────────────────

/** How long the creator waits for Sew to answer with the new channel's id. */
export const CREATE_WAIT_MS = 30_000
const CREATE_POLL_MS = 1_000

/** Wait for a pending channel's Sew id (members by slug); null on timeout. */
export async function waitForChannelId(
  hostKey: string,
  slug: string,
  deps: { sleep: (ms: number) => Promise<void>; now: () => number } = {
    sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
    now: () => Date.now(),
  },
): Promise<string | null> {
  const until = deps.now() + CREATE_WAIT_MS
  while (deps.now() < until) {
    const m = await fetchHomeChatMembers(scopeForChatHost(hostKey), { slug })
    if (m.conv) return m.conv
    await deps.sleep(CREATE_POLL_MS)
  }
  return null
}

export function NewChannelPanel({ home, onBack, onAdded }: { home: Home; onBack: () => void; onAdded?: (address: string) => void }): JSX.Element {
  const servers = useChatServers()
  useServersRead(servers, true)
  const byHost = useHomeChatStore((s) => s.byHost)
  const usable = servers.filter((s) => byHost[s.key]?.status?.on)
  const [hostKey, setHostKey] = useState<string | null>(null)
  const server = usable.find((s) => s.key === hostKey) ?? usable[0] ?? null
  const [name, setName] = useState('')
  const [open, setOpen] = useState(false)
  const [members, setMembers] = useState<Set<string>>(new Set())
  const [state, setState] = useState<{ kind: 'idle' } | { kind: 'busy'; line: string } | { kind: 'error'; line: string }>({ kind: 'idle' })
  const people = server ? (byHost[server.key]?.people?.people ?? []).filter(pickablePerson) : []

  const create = async (): Promise<void> => {
    if (!server || !name.trim()) return
    setState({ kind: 'busy', line: 'Making the channel…' })
    try {
      const made = await createHomeChatChannel(scopeForChatHost(server.key), { name: name.trim(), open, members: [...members] })
      let conv = made.conv
      if (!conv) {
        setState({ kind: 'busy', line: 'Setting up the channel…' })
        conv = await waitForChannelId(server.key, made.slug)
      }
      if (!conv) {
        setState({ kind: 'error', line: 'The channel is still being set up on that server. Add it from Add a channel in a moment.' })
        return
      }
      const address = channelAddress(conv, server.key)
      if (address && useHomesStore.getState().addRow(home.id, { kind: 'channel', address, workspaceId: null, label: name.trim() })) {
        onAdded?.(address)
      }
      setState({ kind: 'idle' })
      onBack()
    } catch (e) {
      setState({ kind: 'error', line: daemonErrorText(e) || String(e) })
    }
  }

  return (
    <>
      <BackHead label="New channel" onBack={onBack} />
      <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-3 py-2 text-[11px]" data-testid="home-chat-new-channel">
        {usable.length === 0 ? (
          <p className="text-[var(--color-text-muted)]">{noChatLine(null)}</p>
        ) : (
          <>
            <label className="flex items-center gap-2">
              <span className="w-14 text-[var(--color-text-muted)]">Server</span>
              <select
                aria-label="Server"
                value={server?.key ?? ''}
                onChange={(e) => {
                  setHostKey(e.target.value)
                  setMembers(new Set())
                }}
                className="min-w-0 flex-1 border border-[var(--color-border)] bg-transparent"
              >
                {usable.map((s) => (
                  <option key={s.key} value={s.key}>
                    {s.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex items-center gap-2">
              <span className="w-14 text-[var(--color-text-muted)]">Name</span>
              <input
                autoFocus
                aria-label="Channel name"
                data-testid="home-chat-new-channel-name"
                value={name}
                maxLength={100}
                onChange={(e) => setName(e.target.value)}
                className="min-w-0 flex-1 border border-[var(--color-border)] bg-transparent px-1.5 py-0.5"
              />
            </label>
            <fieldset className="flex flex-col gap-0.5">
              <label className="flex items-center gap-2">
                <input type="radio" name="hc-visibility" checked={!open} onChange={() => setOpen(false)} data-testid="home-chat-invite-only" />
                Invite-only: only the people in it see it
              </label>
              <label className="flex items-center gap-2">
                <input type="radio" name="hc-visibility" checked={open} onChange={() => setOpen(true)} data-testid="home-chat-open" />
                Open to everyone on this server
              </label>
              <span className="text-[10px] text-[var(--color-text-muted)]">This can't be changed later.</span>
            </fieldset>
            <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">People</div>
            {people.length === 0 ? (
              <p className="text-[var(--color-text-muted)]">Nobody else to add yet.</p>
            ) : (
              people.map((p) => (
                <label key={p.ext} className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={members.has(p.ext)}
                    onChange={(e) =>
                      setMembers((cur) => {
                        const next = new Set(cur)
                        if (e.target.checked) next.add(p.ext)
                        else next.delete(p.ext)
                        return next
                      })
                    }
                  />
                  <span className="truncate">{p.displayName}</span>
                </label>
              ))
            )}
            {state.kind !== 'idle' && (
              <p className={state.kind === 'error' ? 'text-[var(--color-status-needs-you)]' : 'text-[var(--color-text-muted)]'} data-testid="home-chat-new-channel-state">
                {state.line}
              </p>
            )}
            <button
              type="button"
              data-testid="home-chat-new-channel-create"
              disabled={!name.trim() || state.kind === 'busy'}
              onClick={() => void create()}
              className="self-start border border-[var(--color-border)] px-2 py-0.5 text-[var(--color-text-primary)] disabled:opacity-50"
            >
              Make channel
            </button>
          </>
        )}
      </div>
    </>
  )
}
