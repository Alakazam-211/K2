// Home P1 (vs-live H16/H17) — the Add Agent picker, searchable since
// prd-home-picker-and-remote-avatars-v1 S2 (P5–P8; vs-live P23–P26, P32,
// P35, P36).
//
// One button, in the Agents sidebar's Add Workspace spot, opens one
// picker with two choices inside it:
// This server: the CONNECTED server's workspaces (already in the projects
// store — no new fetch), grouped like the Tickets filter (focus groups when
// they're on).
// From a server (desktop): ONE list with a section per saved server (the
// switcher list, plus This computer when the window is on a remote), minus
// the connected one. Every section lists that server's workspaces with ITS
// OWN saved login (host-ops, the Settings → Connections tile path), at
// most 4 at a time. No switch. Each section says whether it is loading,
// offline, needs a sign-in (`signInForManagement` caches a token without
// switching), has no access, or failed.
//
// Both lists are the shared `SearchableAgentList` (the Tickets filter's
// body): a search box over name, path and handle, agent images, ↑/↓ and
// Enter. Agents already on this Home stay in the list, checked, and can't
// be added twice; clicking one does nothing (removing stays on the row's
// right-click). Adding keeps the picker open.
//
// Adding never creates a workspace folder and never calls Add Workspace
// (H7): it only writes a row into `k2.homes.v1`.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useHomesStore, type Home } from '@/stores/homes'
import { HostOpError, hostOpGet } from '@/lib/host-ops'
import { credsForHomeHost } from '@/lib/home-creds'
import { isConnectionLevelError } from '@/lib/remote-retry'
import { hostPool } from '@/lib/host-pool-instance'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  findWorkspaceForRow,
  homeAddress,
  homeHostKey,
  parseHomeAddress,
  workspaceHandle,
} from '@/lib/home-address'
import { hostDisplayAddress } from '@/components/TopBar/ServerSwitcher'
import { groupWorkspacesForFilter } from '@/components/Feedback/WorkspaceFilterDropdown'
import {
  SearchableAgentList,
  matchesAgentRow,
  type AgentListRow,
  type AgentListSection,
} from '@/components/ui/SearchableAgentList'
import { isWebClient } from '@/lib/is-web'
import { putHomeAvatarOnAdd } from '@/lib/home-avatars'

/** A workspace as a server's `projects/list` sends it (the fields the
 *  picker reads; the wire has more). */
export interface ListedWorkspace {
  id: string
  name: string
  handle?: string | null
  path?: string | null
  color?: string | null
  iconUrl?: string | null
}

/** A cached image for a Home address, when the listing has none (S4 fills
 *  it from the local avatar cache, P8/P19). */
export type HomeAvatarLookup = (address: string) => string | null

/** Avatar color for an agent on another server (Q6: no per-agent color;
 *  the same muted color as `OTHER_SERVER_AVATAR_COLOR` on Home rows). */
const PICKER_OTHER_SERVER_COLOR = 'var(--color-text-muted)'

/** P8: only a `data:image/` icon from another server paints; anything else
 *  (an `https://` URL `set-icon` let through, junk) is no image. */
export function listedIconUrl(v: unknown): string | null {
  return typeof v === 'string' && v.startsWith('data:image/') ? v : null
}

function addTo(home: Home, hostKey: string, w: ListedWorkspace): void {
  const h = workspaceHandle(w)
  if (!h) return
  const address = homeAddress(h, hostKey)
  useHomesStore.getState().addRow(home.id, {
    address,
    workspaceId: w.id,
    label: w.name,
  })
  // Cache the row's picture from this listing right away (picker-and-remote-avatars S4).
  putHomeAvatarOnAdd(hostKey, address, w)
}

/** Rename repair from a freshly listed server: rows on `hostKey` whose
 *  handle moved get the new address; labels follow the name. */
export function repairFromList(hostKey: string, workspaces: ListedWorkspace[]): void {
  useHomesStore.getState().repairRows((row) => {
    const p = parseHomeAddress(row.address)
    if (!p || p.host !== hostKey) return null
    const ws = findWorkspaceForRow(workspaces, row)
    if (!ws) return null
    const handle = workspaceHandle(ws)
    if (!handle) return null
    return { address: homeAddress(handle, hostKey), workspaceId: ws.id, label: ws.name }
  })
}

/** One picker row for workspace `w` on `hostKey`; null when it has no
 *  handle (it can't be addressed, so it can't go on a Home). */
function pickerRow(
  home: Home,
  onHome: ReadonlySet<string>,
  hostKey: string,
  w: ListedWorkspace,
  value: string,
  avatar: AgentListRow['avatar'],
): AgentListRow | null {
  const handle = workspaceHandle(w)
  if (!handle) return null
  const taken = onHome.has(homeAddress(handle, hostKey))
  return {
    value,
    label: w.name,
    detail: handle,
    avatar,
    state: taken ? 'checked' : 'pickable',
    title: taken ? `Already on ${home.name}` : `Add ${w.name} to ${home.name}`,
    keywords: [w.path ?? '', handle],
  }
}

// The Agents sidebar's dropdown look (FocusGroupDropdown), opening upward
// from the bottom bar. A column, so the list body scrolls under its search.
const panelClass =
  'absolute bottom-full left-3 right-3 z-50 mb-0.5 max-h-[60vh] flex flex-col bg-[var(--color-bg)] border border-[var(--color-border)] shadow-xl py-0.5'
const itemClass =
  'no-drag w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px] transition-colors cursor-pointer text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
const headClass = 'px-3 pt-1.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]'
const listClass = 'flex-1 min-h-0 overflow-y-auto py-1'
const statusClass = 'px-2 py-1.5 text-[11px] text-[var(--color-text-muted)]'
const statusButtonClass =
  'no-drag px-2 py-0.5 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.06] cursor-pointer flex-shrink-0'

export function connectedServerLabel(active: 'local' | ConnectHost): string {
  return active === 'local' ? 'This computer' : active.label
}

function BackHead({ label, onBack }: { label: string; onBack: () => void }): React.JSX.Element {
  return (
    <button type="button" className={`${itemClass} ${headClass} normal-case flex-shrink-0`} onClick={onBack}>
      ‹ {label}
    </button>
  )
}

function useOnHome(home: Home): ReadonlySet<string> {
  return useMemo(() => new Set(home.rows.map((r) => r.address)), [home.rows])
}

function ThisServerList({ home, onBack }: { home: Home; onBack: () => void }): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const projects = useProjectsStore((s) => s.projects)
  const focusGroups = useFocusGroupsStore((s) => s.focusGroups)
  const focusGroupsEnabled = useFocusGroupsStore((s) => s.focusGroupsEnabled)
  const hostKey = activeHomeHostKey(activeHost)
  const onHome = useOnHome(home)
  const [query, setQuery] = useState('')

  // P5: grouped the way Tickets groups them; the list does the search
  // (name, path, handle — P26), so the grouping gets no query.
  const sections = useMemo((): AgentListSection[] => {
    const out: AgentListSection[] = []
    const byId = new Map(projects.map((p) => [p.id, p]))
    for (const s of groupWorkspacesForFilter(projects, focusGroups, focusGroupsEnabled, '')) {
      const rows: AgentListRow[] = []
      for (const fw of s.workspaces) {
        // The full store row: the grouping keeps only the filter's fields,
        // and the address needs the handle.
        const p = byId.get(fw.id)
        if (!p) continue
        const r = pickerRow(home, onHome, hostKey, p, p.id, {
          kind: 'workspace',
          path: p.path,
          name: p.name,
          color: p.color,
          id: p.id,
          iconUrl: p.iconUrl,
          // The connected server is the one `primaryScope()` asks.
          fetchIcon: true,
        })
        if (r) rows.push(r)
      }
      if (rows.length > 0) out.push({ key: s.key, label: s.label, color: s.color, rows })
    }
    return out
  }, [projects, focusGroups, focusGroupsEnabled, home, onHome, hostKey])

  const onPick = (value: string): void => {
    const ws = useProjectsStore.getState().projects.find((p) => p.id === value)
    if (ws) addTo(home, hostKey, ws)
  }

  return (
    <>
      <BackHead label={`Agents on ${connectedServerLabel(activeHost)}`} onBack={onBack} />
      <SearchableAgentList
        sections={sections}
        query={query}
        onQuery={setQuery}
        onPick={onPick}
        matches={matchesAgentRow}
        placeholder="Search agents..."
        emptyText={query.trim() ? 'No agents match' : 'No agents on this server'}
        ariaLabel={`Agents on ${connectedServerLabel(activeHost)}`}
        listClassName={listClass}
        autoFocus
      />
    </>
  )
}

type PickerMode = 'menu' | 'this' | 'server'

/** The one Add Agent picker: This server, or From a server (desktop).
 *  `avatarFor` supplies a cached image for a row on another server when
 *  its listing has none (S4's avatar cache). */
export function AddAgentPicker({
  home,
  avatarFor,
}: {
  home: Home
  avatarFor?: HomeAvatarLookup
}): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const [mode, setMode] = useState<PickerMode>('menu')
  const web = isWebClient()
  return (
    <div className={panelClass} role="dialog" aria-label="Add Agent">
      {mode === 'menu' && (
        <>
          <div className={headClass}>Add an agent to {home.name}</div>
          <button type="button" className={itemClass} onClick={() => setMode('this')}>
            <span className="truncate flex-1">This server</span>
            <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">{connectedServerLabel(activeHost)}</span>
            <span className="flex-shrink-0 text-[var(--color-text-muted)]">›</span>
          </button>
          {!web && (
            <button type="button" className={itemClass} onClick={() => setMode('server')}>
              <span className="truncate flex-1">From a server</span>
              <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">saved servers</span>
              <span className="flex-shrink-0 text-[var(--color-text-muted)]">›</span>
            </button>
          )}
        </>
      )}
      {mode === 'this' && <ThisServerList home={home} onBack={() => setMode('menu')} />}
      {mode === 'server' && <FromServerList home={home} avatarFor={avatarFor} onBack={() => setMode('menu')} />}
    </div>
  )
}

// ── From a server ─────────────────────────────────────────────────────────

/** One server section's listing state (P6, vs-live P23). */
export type ServerListing =
  | { kind: 'loading' }
  | { kind: 'signin'; host: ConnectHost }
  | { kind: 'offline' }
  | { kind: 'no-access' }
  | { kind: 'error'; message: string }
  | { kind: 'ok'; workspaces: ListedWorkspace[] }

/** P36: each server's last good listing, for 60 s, so reopening the panel
 *  paints at once without refetching every server. */
export const LISTING_MEMO_MS = 60_000
/** P24: a pool check this recent that found the server offline shows
 *  "Offline" without a fetch. */
export const POOL_OFFLINE_FRESH_MS = 60_000
/** P6: listings in flight at once. */
export const LISTING_CONCURRENCY = 4

const listingMemo = new Map<string, { at: number; workspaces: ListedWorkspace[] }>()

export function __resetAddPickerListingsForTests(): void {
  listingMemo.clear()
}

/** P23: what a failed `projects/list` means for the section. */
export function listingForError(err: unknown, host: ConnectHost | null): ServerListing {
  if (err instanceof HostOpError) {
    if (err.status === 401) return host ? { kind: 'signin', host } : { kind: 'error', message: err.message }
    if (err.status === 403) return { kind: 'no-access' }
    return { kind: 'error', message: err.message }
  }
  if (isConnectionLevelError(err) || (err instanceof Error && err.name === 'TimeoutError')) {
    return { kind: 'offline' }
  }
  return { kind: 'error', message: err instanceof Error ? err.message : String(err) }
}

/** Run `fn` with at most `limit` running at once. */
function createLimiter(limit: number): <T>(fn: () => Promise<T>) => Promise<T> {
  let running = 0
  const waiting: (() => void)[] = []
  return async <T,>(fn: () => Promise<T>): Promise<T> => {
    if (running >= limit) await new Promise<void>((r) => waiting.push(r))
    running++
    try {
      return await fn()
    } finally {
      running--
      waiting.shift()?.()
    }
  }
}

interface ServerChoice {
  key: string
  label: string
  sub: string
}

function SectionStatus({
  listing,
  label,
  onRetry,
}: {
  listing: ServerListing
  label: string
  onRetry: () => void
}): React.JSX.Element | null {
  switch (listing.kind) {
    case 'loading':
      return <p className={statusClass}>Loading agents…</p>
    case 'signin':
      return (
        <div className="flex items-center gap-2 px-2 py-1.5" data-listing="signin">
          <p className="flex-1 text-[11px] text-[var(--color-text-secondary)]">Sign in to {label} to list its agents.</p>
          <button
            type="button"
            className="no-drag px-2 py-0.5 text-[11px] bg-[var(--color-accent)] text-[var(--color-on-accent)] hover:opacity-90 cursor-pointer flex-shrink-0"
            onClick={() => useConnectHostStore.getState().signInForManagement(listing.host)}
          >
            Sign in
          </button>
        </div>
      )
    case 'offline':
      return (
        <div className="flex items-center gap-2 px-2 py-1.5" data-listing="offline">
          <p className="flex-1 text-[11px] text-[var(--color-text-muted)]">Offline</p>
          <button type="button" className={statusButtonClass} onClick={onRetry}>
            Retry
          </button>
        </div>
      )
    case 'no-access':
      return (
        <p className={statusClass} data-listing="no-access">
          Your login can’t list agents here
        </p>
      )
    case 'error':
      return (
        <div className="flex items-center gap-2 px-2 py-1.5" data-listing="error">
          <p className="flex-1 min-w-0 break-words text-[11px] text-[var(--color-status-error-soft)]">
            Couldn’t list agents: {listing.message}
          </p>
          <button type="button" className={statusButtonClass} onClick={onRetry}>
            Retry
          </button>
        </div>
      )
    case 'ok':
      return null
  }
}

function FromServerList({
  home,
  avatarFor,
  onBack,
}: {
  home: Home
  avatarFor?: HomeAvatarLookup
  onBack: () => void
}): React.JSX.Element {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectedKey = activeHomeHostKey(activeHost)
  const onHome = useOnHome(home)
  const [query, setQuery] = useState('')
  const [listings, setListings] = useState<Record<string, ServerListing>>({})
  const limiter = useRef(createLimiter(LISTING_CONCURRENCY))
  const seq = useRef(new Map<string, number>())
  const alive = useRef(true)

  // The saved servers, deduped by row host key, minus the connected one;
  // This computer first, then the rest by label.
  const servers = useMemo((): ServerChoice[] => {
    const out: ServerChoice[] = []
    if (connectedKey !== LOCAL_HOME_HOST) out.push({ key: LOCAL_HOME_HOST, label: 'This computer', sub: 'local' })
    const seen = new Set(out.map((s) => s.key))
    const sorted = [...hosts].sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }))
    for (const h of sorted) {
      const key = homeHostKey(h)
      if (key === connectedKey || seen.has(key)) continue
      seen.add(key)
      out.push({ key, label: h.label, sub: hostDisplayAddress(h) })
    }
    return out
  }, [hosts, connectedKey])

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  const setListing = useCallback((key: string, n: number, listing: ServerListing) => {
    if (!alive.current || seq.current.get(key) !== n) return
    setListings((prev) => ({ ...prev, [key]: listing }))
  }, [])

  /** List one server. `force` (Retry, a fresh sign-in) skips the memo and
   *  the pool's recent offline verdict. */
  const load = useCallback(
    (key: string, force: boolean): void => {
      const n = (seq.current.get(key) ?? 0) + 1
      seq.current.set(key, n)
      if (!force) {
        const memo = listingMemo.get(key)
        if (memo && Date.now() - memo.at < LISTING_MEMO_MS) {
          setListing(key, n, { kind: 'ok', workspaces: memo.workspaces })
          return
        }
        const entry = hostPool.store.getState().entries[key]
        if (entry?.reach === 'offline' && entry.checkedAt !== null && Date.now() - entry.checkedAt < POOL_OFFLINE_FRESH_MS) {
          setListing(key, n, { kind: 'offline' })
          return
        }
      }
      setListing(key, n, { kind: 'loading' })
      void limiter.current(async () => {
        if (!alive.current || seq.current.get(key) !== n) return
        const saved = useConnectHostStore.getState().hosts
        const host = key === LOCAL_HOME_HOST ? null : (saved.find((h) => homeHostKey(h) === key) ?? null)
        const c = await credsForHomeHost(key, saved)
        if (c.kind === 'unsaved') {
          setListing(key, n, { kind: 'error', message: 'That server is no longer saved.' })
          return
        }
        if (c.kind === 'unreachable') {
          setListing(key, n, { kind: 'offline' })
          return
        }
        if (!c.creds.token) {
          setListing(key, n, host ? { kind: 'signin', host } : { kind: 'error', message: 'That server is no longer saved.' })
          return
        }
        try {
          const raw = await hostOpGet<unknown>(c.creds, 'projects/list')
          if (!alive.current || seq.current.get(key) !== n) return
          const workspaces = Array.isArray(raw) ? (raw as ListedWorkspace[]) : []
          repairFromList(key, workspaces)
          listingMemo.set(key, { at: Date.now(), workspaces })
          setListing(key, n, { kind: 'ok', workspaces })
        } catch (err) {
          setListing(key, n, listingForError(err, host))
        }
      })
    },
    [setListing],
  )

  // Every server lists when the panel opens (or a server is added).
  const serverKeys = servers.map((s) => s.key).join('\n')
  useEffect(() => {
    for (const key of serverKeys ? serverKeys.split('\n') : []) {
      if (seq.current.has(key)) continue
      load(key, false)
    }
  }, [serverKeys, load])

  // A token landing (a sign-in, or a new one after a 401) re-lists that
  // server, skipping the memo.
  const tokens = useRef<Map<string, string> | null>(null)
  useEffect(() => {
    const next = new Map<string, string>()
    for (const h of hosts) {
      const key = homeHostKey(h)
      if (h.token.length > 0 && !next.has(key)) next.set(key, h.token)
    }
    const prev = tokens.current
    tokens.current = next
    if (!prev) return
    for (const [key, token] of next) {
      if (prev.get(key) !== token && servers.some((s) => s.key === key)) load(key, true)
    }
  }, [hosts, servers, load])

  const valueMap = useRef(new Map<string, { hostKey: string; w: ListedWorkspace }>())
  const hasQuery = query.trim().length > 0
  const sections = useMemo((): AgentListSection[] => {
    const values = new Map<string, { hostKey: string; w: ListedWorkspace }>()
    const out = servers.map((s): AgentListSection => {
      const listing = listings[s.key] ?? { kind: 'loading' }
      const rows: AgentListRow[] = []
      if (listing.kind === 'ok') {
        const sorted = [...listing.workspaces].sort((a, b) => a.name.localeCompare(b.name))
        for (const w of sorted) {
          const value = `${w.id}@${s.key}`
          const handle = workspaceHandle(w)
          const address = handle ? homeAddress(handle, s.key) : null
          const r = pickerRow(home, onHome, s.key, w, value, {
            kind: 'workspace',
            // Its own cache key: ProjectAvatar caches by path, and this path
            // means nothing on the connected server.
            path: `home-add:${s.key}:${w.path ?? w.id}`,
            name: w.name,
            color: PICKER_OTHER_SERVER_COLOR,
            id: w.id,
            iconUrl: listedIconUrl(w.iconUrl) ?? (address && avatarFor ? avatarFor(address) : null),
            fetchIcon: false,
          })
          if (!r) continue
          values.set(value, { hostKey: s.key, w })
          rows.push(r)
        }
      }
      let status: React.ReactNode = undefined
      if (listing.kind !== 'ok') {
        status = <SectionStatus listing={listing} label={s.label} onRetry={() => load(s.key, true)} />
      } else if (!hasQuery && rows.length === 0) {
        status = <p className={statusClass}>No agents</p>
      } else if (!hasQuery && rows.every((r) => r.state === 'checked')) {
        status = <p className={statusClass}>Every agent here is already on this Home</p>
      }
      return { key: s.key, label: s.label, count: listing.kind === 'ok' ? undefined : null, status, rows }
    })
    valueMap.current = values
    return out
  }, [servers, listings, home, onHome, hasQuery, avatarFor, load])

  const onPick = (value: string): void => {
    const hit = valueMap.current.get(value)
    if (hit) addTo(home, hit.hostKey, hit.w)
  }

  return (
    <>
      <BackHead label="Saved servers" onBack={onBack} />
      {servers.length === 0 ? (
        <p className="px-3 py-2 text-[11px] text-[var(--color-text-muted)]">
          No other saved servers. Add one in Settings → Connections.
        </p>
      ) : (
        <SearchableAgentList
          sections={sections}
          query={query}
          onQuery={setQuery}
          onPick={onPick}
          matches={matchesAgentRow}
          placeholder="Search agents on saved servers..."
          emptyText="No agents match"
          ariaLabel="Agents on saved servers"
          listClassName={listClass}
          autoFocus
        />
      )}
    </>
  )
}
