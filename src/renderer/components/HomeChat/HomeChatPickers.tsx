// People and channels on Home: the pickers (prd-home-people-channels-v1
// §6, §7; slice S6). They are the People and Channels sections of the one
// Add to Home modal (`AddToHomeModal`), beside its Agents section. Each
// section is presentational over the Home chat store: what a pick adds goes
// out through `onAddRow` (the modal writes the Home row), so K2 Design can
// restyle or rehouse them freely.
//
//   - Add a person: one list with a section per server you're signed in
//     to, each asked with that server's own login (`/cli/home-chat/people`).
//     Only people: owner, admin, member and Voice & Chat only logins you
//     share the server with. Never App users, disabled logins or agents (the
//     daemon lists none of them; the client drops anything else too). A
//     person whose chat is still being set up shows "Setting up…".
//     Adding writes a `person` row; the DM is opened on the first open.
//     A match on another server asks "Same person as …? Keep one row." (§6.3).
//   - New channel: a server (when there is a choice), a name, who can see
//     it (Invite only, the default, or Open: a segmented control), and
//     people; the Settings field style, K2's Input, dropdown and checkbox. The daemon makes it over mgmt with a random slug; the row goes
//     on the Home once Sew answers with the channel's id.
//   - Add a channel: a server's text channels you can see (browse), joined
//     on open when it is open and you aren't in it yet.
//
// A server where you are Voice & Chat only is listed here, for people and
// channels only (HP-14). A Mac server, or one with chat off, says "No chat
// on this server".

import { useEffect, useId, useMemo, useRef, useState, type JSX } from 'react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import type { Home, HomeRow } from '@/stores/homes'
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
import { Input } from '@/components/ui/Input'
import { Checkbox } from '@/components/ui/Checkbox'
import { Button } from '@/components/ui/Button'
import { Segmented } from '@/components/ui/Segmented'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { HomeChatChannelGlyph, HomeChatPersonAvatar } from './HomeChatGlyphs'

const itemClass =
  'no-drag w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px] transition-colors cursor-pointer text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)]'
const headClass = 'px-3 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]'
const listClass = 'flex-1 min-h-0 overflow-y-auto py-1'

/** Why a server has no one to add, and what to do, from a status read
 *  (`unreachable`: the status could not be read at all). */
export function noChatLine(reason: string | null, o: { unreachable?: boolean } = {}): string {
  if (o.unreachable) return "Couldn't reach chat on this server. Check it's online, then try again."
  if (reason === 'mac') return 'No chat here: chat runs on Linux servers. Use a Linux server to chat with people.'
  if (reason === 'space_pending') return 'Chat is still being set up on this server.'
  return 'Chat is off on this server. An owner or admin can turn it on.'
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

/** What every section takes: the Home it adds to, the add itself (true
 *  when the row went on), and an optional back row (a menu that holds it). */
export interface HomeChatSectionProps {
  home: Home
  onAddRow: (row: HomeRow) => boolean
  onBack?: () => void
  /** Focus the section's first field on mount (default). Off when ←/→ on
   *  Add to Home's tab bar showed it, so focus stays on the tabs. */
  autoFocus?: boolean
}

function BackHead({ label, onBack }: { label: string; onBack?: () => void }): JSX.Element | null {
  if (!onBack) return null
  return (
    <button type="button" className={`${itemClass} ${headClass} normal-case flex-shrink-0`} onClick={onBack}>
      ‹ {label}
    </button>
  )
}

/** A person's avatar in a list row (20px, the list's agent avatar size). */
function personAvatar(p: HomeChatPerson): AgentListRow['avatar'] {
  return { kind: 'node', node: <HomeChatPersonAvatar username={p.username} name={p.displayName} picture={p.picture} size={20} /> }
}

/** A channel's glyph in a list row: `#` open, a lock invite-only. */
function channelGlyph(inviteOnly: boolean): AgentListRow['avatar'] {
  return {
    kind: 'node',
    node: (
      <span aria-hidden="true" className="inline-flex h-5 w-5 flex-shrink-0 items-center justify-center text-[var(--color-text-secondary)]">
        <HomeChatChannelGlyph inviteOnly={inviteOnly} size={14} />
      </span>
    ),
  }
}

/** A server section's status line while it has no list to show (none: the
 *  list shows). */
function sectionStatus(st: HomeChatHostState | undefined, needPeople: boolean): JSX.Element | undefined {
  if (!st || (!st.status && !st.statusError)) return <AgentListStatus text="Loading…" />
  if (st.statusError) return <AgentListStatus marker="offline" text={noChatLine(null, { unreachable: true })} />
  if (st.status && !st.status.on) return <AgentListStatus marker="no-access" text={noChatLine(st.status.reason)} />
  if (needPeople && !st.people) return <AgentListStatus text="Loading people…" />
  return undefined
}

// ── Add a person ──────────────────────────────────────────────────────

/** §6.3's question when a person may already be on this Home from another
 *  server: "Same person" keeps their one row, "Different person" adds a
 *  second, and Cancel (Esc, ×, a press outside) does neither: it adds
 *  nothing. Resolves 'same', 'different' or null. */
export function askSamePerson(name: string, theirName: string, theirServer: string): Promise<'same' | 'different' | null> {
  return useConfirmDialogStore
    .getState()
    .choose({
      title: `Is ${name} the same person as ${theirName} on ${theirServer}?`,
      message: `Same person keeps one row: ${theirName}'s row stays, and its menu's “Also on” switches servers. Different person adds a second row.`,
      choices: [
        { id: 'different', label: 'Different person' },
        { id: 'same', label: 'Same person', primary: true },
      ],
    })
    .then((id) => (id === 'same' || id === 'different' ? id : null))
}

/** The person rows of one server (people only: never agents or App users). */
function listedPeople(st: HomeChatHostState | undefined): HomeChatPerson[] {
  return (st?.people?.people ?? []).filter((p) => /^(k2:owner|k2:connect:)/.test(p.ext))
}

export function AddPersonPanel({ home, onAddRow, onBack, autoFocus = true }: HomeChatSectionProps): JSX.Element {
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
              avatar: personAvatar(p),
              state: taken ? 'checked' : ready ? 'pickable' : 'disabled',
              title: taken
                ? `Already in ${home.name}`
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
      const answer = await askSamePerson(person.displayName, match.person.displayName, serverLabelFor(match.hostKey, hosts))
      // Same person: their row stays (its "Also on" switches servers).
      // Cancel: nothing at all. Only "Different person" adds a second row.
      if (answer !== 'different') return
    }
    onAddRow({ kind: 'person', address, workspaceId: null, label: person.displayName })
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
        autoFocus={autoFocus}
      />
    </>
  )
}

// ── Add an existing channel ───────────────────────────────────────────

type ChannelListing = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ok'; channels: HomeChatChannel[] }

export function AddChannelPanel({ home, onAddRow, onBack, autoFocus = true }: HomeChatSectionProps): JSX.Element {
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
          detail: c.inviteOnly ? 'Invite only' : 'Open',
          avatar: channelGlyph(c.inviteOnly),
          state: taken ? 'checked' : 'pickable',
          title: taken ? `Already in ${home.name}` : `Add ${c.name} to ${home.name}`,
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
    onAddRow({ kind: 'channel', address, workspaceId: null, label: c.name })
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
        autoFocus={autoFocus}
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

/** `onDone`: the channel is made and on the Home (the modal closes). */
export function NewChannelPanel({ onAddRow, onBack, onDone, autoFocus = true }: HomeChatSectionProps & { onDone?: () => void }): JSX.Element {
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
  const formId = useId()

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
      if (address) onAddRow({ kind: 'channel', address, workspaceId: null, label: name.trim() })
      setState({ kind: 'idle' })
      setName('')
      setMembers(new Set())
      onDone?.()
    } catch (e) {
      setState({ kind: 'error', line: daemonErrorText(e) || String(e) })
    }
  }

  return (
    <>
      <BackHead label="New channel" onBack={onBack} />
      <div className="flex min-h-0 flex-1 flex-col" data-testid="home-chat-new-channel">
        {usable.length === 0 ? (
          <p className={`${SETTING_ROW_HINT} px-5 py-2`}>{noChatLine(null)}</p>
        ) : (
          <>
            <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 pt-2 pb-3">
              {usable.length > 1 && (
                <div className="flex flex-col gap-1.5" data-new-channel-field="server">
                  <span className={SETTING_ROW_LABEL} id={`${formId}-server`}>
                    Server
                  </span>
                  <SettingDropdown
                    value={server?.key ?? ''}
                    options={usable.map((s) => ({ value: s.key, label: s.label }))}
                    onChange={(key) => {
                      setHostKey(key)
                      setMembers(new Set())
                    }}
                    ariaLabel="Server"
                    menuAlign="left"
                    fullWidth
                  />
                </div>
              )}
              <label className="flex flex-col gap-1.5" data-new-channel-field="name">
                <span className={SETTING_ROW_LABEL}>Name</span>
                <Input
                  autoFocus={autoFocus}
                  aria-label="Channel name"
                  data-testid="home-chat-new-channel-name"
                  value={name}
                  maxLength={100}
                  placeholder="e.g. launch"
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && name.trim() && state.kind !== 'busy') {
                      e.preventDefault()
                      void create()
                    }
                  }}
                  className="w-full"
                />
              </label>
              <div className="flex flex-col gap-1.5" data-new-channel-field="privacy" data-k2-line="">
                <span className={SETTING_ROW_LABEL}>Who can see it</span>
                <div className="flex justify-start">
                  <Segmented
                    aria-label="Who can see it"
                    value={open ? 'open' : 'invite'}
                    onChange={(v) => setOpen(v === 'open')}
                    options={[
                      { value: 'invite', label: 'Invite only', buttonProps: { 'data-testid': 'home-chat-invite-only' } },
                      { value: 'open', label: 'Open', buttonProps: { 'data-testid': 'home-chat-open' } },
                    ]}
                  />
                </div>
                <span className={SETTING_ROW_HINT} data-new-channel-privacy-hint="">
                  {open ? 'Everyone on this server can find it and join.' : 'Only the people in it see it.'} This can't be changed later.
                </span>
              </div>
              <div className="flex flex-col gap-1" data-new-channel-field="people">
                <span className={SETTING_ROW_LABEL}>People</span>
                {people.length === 0 ? (
                  <p className={`m-0 ${SETTING_ROW_HINT}`}>Nobody else to add yet.</p>
                ) : (
                  <div className="-mx-2 flex flex-col">
                    {people.map((p) => (
                      <label
                        key={p.ext}
                        className="flex min-h-[36px] cursor-pointer items-center gap-2.5 rounded-[var(--radius-selector)] px-2 py-1 hover:bg-[var(--color-row-hover)]"
                      >
                        <Checkbox
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
                        <HomeChatPersonAvatar username={p.username} name={p.displayName} picture={p.picture} size={20} />
                        <span className="min-w-0 flex-1 truncate type-name text-[var(--color-text-primary)]">{p.displayName}</span>
                      </label>
                    ))}
                  </div>
                )}
              </div>
            </div>
            <div className="flex flex-shrink-0 items-center gap-3 border-t border-[var(--color-border)] px-5 py-3" data-new-channel-foot="">
              <p
                className={`m-0 min-w-0 flex-1 type-hint ${state.kind === 'error' ? 'text-[var(--color-status-error-text)]' : 'text-[var(--color-text-secondary)]'}`}
                data-testid="home-chat-new-channel-state"
                role={state.kind === 'error' ? 'alert' : 'status'}
              >
                {state.kind === 'idle' ? '' : state.line}
              </p>
              <Button
                variant="accent"
                data-testid="home-chat-new-channel-create"
                disabled={!name.trim() || state.kind === 'busy'}
                onClick={() => void create()}
              >
                Make channel
              </Button>
            </div>
          </>
        )}
      </div>
    </>
  )
}
