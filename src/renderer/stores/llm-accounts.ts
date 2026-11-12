// LLM tokens (Settings → LLMs → Tokens, `k2 llm tokens`), per server. People see
// "tokens": subscriptions (a CLI sign-in) and API tokens (a key). The
// internals (routes, types, DB) keep the older "accounts"/"login" names.
//
// One token per tool is the server default ("the pool" internally); the
// others wait in the daemon's store (`~/.k2/llm-accounts`). The daemon owns every token and
// returns metadata only. This store renders that truth and sends
// gestures; it refetches on the app-level `llm_accounts_changed` event and
// never polls the list. A login sheet polls `login/status` while it is
// open, so a phone can sign in without reading a terminal.

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onActiveHostChange } from '@/stores/connect-host'
import { isWebClient } from '@/lib/is-web'
import { onLlmAccountsChanged, onSubscriptionUsageChanged } from '@/stores/session-events'
import type { HarnessUsage } from '@/lib/subscription-usage'
import { useSubscriptionUsageStore } from '@/stores/subscription-usage'

export type AccountState = 'not_set_up' | 'signing_in' | 'signed_in' | 'needs_login' | 'unknown'

/** A pin: a workspace (all its agents) or one session runs on a login. */
export interface LlmPin {
  scopeKind: 'workspace' | 'session'
  scopeId: string
  tool: string
  accountId: string
  /** Workspace name/handle, or the session key (a conversation pin: the
   *  chat's name or the start of its id). */
  label: string
  /** When the pick was made (unix seconds); the newer of a chat's two
   *  rows wins. Older daemons don't send it. */
  createdAt?: number
}

export type LoginKind = 'subscription' | 'api_key'

export interface LlmAccount {
  id: string
  tool: string
  label: string
  /** `subscription` (a CLI sign-in) or `api_key` (billed per token). */
  kind?: LoginKind | string
  billedPerToken?: boolean
  /** Where this login is pinned (empty = only the pool may use it). */
  pinnedTo?: LlmPin[]
  /** A running pinned session uses this login's slot. */
  inUse?: boolean
  active: boolean
  state: AccountState | string
  detail: string | null
  email?: string | null
  org?: string | null
  plan: string | null
  expiresAt: number | null
  refreshedAt: number | null
  lastUsedAt: number | null
  createdAt: number
  createdBy: string | null
  /** K2 saved this token from a sign-in someone made outside K2 (a
   *  `/login` in a terminal). Additive (0.46.6); absent on older daemons. */
  addedOutsideK2?: boolean
  usage: HarnessUsage | null
  usageCheckedAt: number | null
}

export interface LlmTool {
  tool: string
  display: string
  supported: boolean
  /** Sign-in (subscription) logins supported; false for API-key-only tools. */
  subscription: boolean
  /** API-key logins supported. */
  apiKeys: boolean
  activeId: string | null
  /** The subscription login in the tool's live store (differs from
   *  activeId while an API key is the pool's active login). */
  liveAccountId: string | null
  /** When the current server default was set (unix seconds); null on
   *  older daemons and for tools with no default. */
  switchedAt: number | null
  /** Who set it (`owner-token`, `login:<user>`); people only. */
  switchedBy: string | null
  pins: LlmPin[]
  loginMethod: 'temp_home' | 'live_swap' | null
  accounts: LlmAccount[]
}

export type LoginMode = 'this_computer' | 'other_device'

export type LoginState =
  | 'signing_in'
  | 'waiting_for_browser'
  | 'waiting_for_code'
  | 'verifying'
  | 'signed_in'
  | 'failed'
  | 'cancelled'
  | 'timed_out'

export interface LlmLogin {
  loginId: string
  accountId: string
  tool: string
  label: string
  mode: LoginMode
  method: 'temp_home' | 'live_swap'
  state: LoginState | string
  /** The whole sign-in URL. Open and Copy always use this, never DOM text. */
  url: string | null
  /** The URL's host ("claude.ai"), for "Sign in at …". Older daemons omit it. */
  urlHost?: string | null
  code: string | null
  error: string | null
  /** The visible terminal as logical lines (a long URL stays on one line). */
  screen: string[]
  startedAt: number
  done: boolean
  banner: string | null
  offerMakeActive: boolean
}

/** A Claude keychain item macOS won't let Claude read without a password
 *  dialog (left by K2 0.45.1/0.45.2), and the one-line Terminal fix. */
export interface KeychainRepair {
  service: string
  account: string
  /** "server default" or `token "<label>"`. */
  holds: string
  partitions: string[]
  command: string
}

export interface LlmAccountsDoc {
  tools: LlmTool[]
  logins: LlmLogin[]
  airgap: boolean
  switchNote: string
  keychainRepairs: KeychainRepair[]
  /** May THIS viewer change the server default (switch / next)? Admin
   *  and above only (Rosson 2026-10-20); the daemon decides. */
  canChangeDefault: boolean
  /** Why not, shown on the disabled control. */
  changeDefaultReason: string | null
}

export interface AccountsEntry {
  doc: LlmAccountsDoc | null
  error: string | null
  /** The server's own reason for a failed load, for the page to show
   *  quietly under its own words; null when `error` already says it all
   *  (an older server, a role refusal) or nothing failed. */
  reason?: string | null
}

/** Tools with a wallet in this version (Gemini: API keys only). */
export const WALLET_TOOLS = ['claude', 'codex', 'grok', 'gemini'] as const

export const STATE_LABELS: Record<string, string> = {
  signed_in: 'Signed in',
  needs_login: 'Needs sign-in',
  signing_in: 'Signing in…',
  not_set_up: 'Not set up',
  unknown: 'Unknown',
}

export const LOGIN_STATE_LABELS: Record<string, string> = {
  signing_in: 'Starting sign-in…',
  waiting_for_browser: 'Waiting for you to sign in in the browser',
  waiting_for_code: 'Paste the code from the sign-in page',
  verifying: 'Checking the sign-in…',
  signed_in: 'Signed in',
  failed: 'Sign-in failed',
  cancelled: 'Cancelled',
  timed_out: 'Sign-in timed out',
}

function rawMessage(e: unknown): string {
  return (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
}

/** A server older than 0.45.0 has no token routes: a login gets
 *  `route_unclassified` (404), the owner token `route not found`. */
const NO_WALLET = /route_unclassified|route not found/i
/** The route policy refused this login's role (`daemon-cli` keeps only the
 *  `error` string of `{"error":"role_required",…}`). */
const ROLE_REFUSED = /role_required/

/** The daemon's `{"error":{"code","hint"}}` → a readable line. */
export function errorText(e: unknown): string {
  const raw = rawMessage(e)
  if (NO_WALLET.test(raw)) return 'This server runs a K2 older than 0.45.0, which has no LLM tokens.'
  if (ROLE_REFUSED.test(raw)) return "Your login on this server isn't allowed to change LLM tokens."
  try {
    const parsed = JSON.parse(raw) as { error?: { hint?: unknown; code?: unknown } }
    const err = parsed?.error
    if (err && typeof err === 'object') {
      if (typeof err.hint === 'string' && err.hint) return err.hint
      if (typeof err.code === 'string') return err.code
    }
  } catch {
    /* not JSON: the message as-is */
  }
  return raw
}

function asArray<T>(v: unknown): T[] {
  return Array.isArray(v) ? (v as T[]) : []
}

/** Parse the list route defensively (a daemon without the wallet answers
 *  404; a malformed body becomes an empty doc, never a crash). */
export function parseAccountsDoc(raw: unknown): LlmAccountsDoc {
  const o = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>
  const tools = asArray<Record<string, unknown>>(o.tools).map((t) => {
    const supported = t.supported === true
    return {
      tool: String(t.tool ?? ''),
      display: String(t.display ?? t.tool ?? ''),
      supported,
      // Older daemons (before API keys) sent neither flag: supported meant sign-in.
      subscription: typeof t.subscription === 'boolean' ? t.subscription : supported,
      apiKeys: t.apiKeys === true,
      activeId: typeof t.activeId === 'string' ? t.activeId : null,
      liveAccountId: typeof t.liveAccountId === 'string' ? t.liveAccountId : null,
      switchedAt: typeof t.switchedAt === 'number' && Number.isFinite(t.switchedAt) ? t.switchedAt : null,
      switchedBy: typeof t.switchedBy === 'string' ? t.switchedBy : null,
      pins: asArray<LlmPin>(t.pins),
      loginMethod: (t.loginMethod === 'live_swap' || t.loginMethod === 'temp_home' ? t.loginMethod : null) as LlmTool['loginMethod'],
      accounts: asArray<LlmAccount>(t.accounts),
    }
  })
  return {
    tools,
    logins: asArray<LlmLogin>(o.logins),
    airgap: o.airgap === true,
    switchNote: typeof o.switchNote === 'string' ? o.switchNote : '',
    // An older daemon doesn't send it: it still lets every login switch.
    canChangeDefault: o.canChangeDefault !== false,
    changeDefaultReason: typeof o.changeDefaultReason === 'string' ? o.changeDefaultReason : null,
    keychainRepairs: asArray<Record<string, unknown>>(o.keychainRepairs)
      .filter((r) => typeof r.command === 'string' && r.command !== '')
      .map((r) => ({
        service: String(r.service ?? ''),
        account: String(r.account ?? ''),
        holds: String(r.holds ?? ''),
        partitions: asArray<unknown>(r.partitions).map(String),
        command: String(r.command),
      })),
  }
}

/** The server's reason for a failed load, or null when `loadErrorText`
 *  already explains it in its own words. */
export function loadErrorReason(e: unknown): string | null {
  const raw = rawMessage(e)
  if (NO_WALLET.test(raw) || ROLE_REFUSED.test(raw)) return null
  return errorText(e)
}

/** The server as running text names it: "this computer" (or "this
 *  server" on hosted web, which is one server and whose primary scope's
 *  label reads "This computer"), else the saved server's own name. */
export function serverInSentence(scope: ServerScope): string {
  if (isWebClient()) return 'this server'
  return scope.isRemote ? scope.label : 'this computer'
}

/** A load failure as a person reads it, naming the server. */
export function loadErrorText(scope: ServerScope, e: unknown): string {
  const name = serverInSentence(scope)
  const Name = name.charAt(0).toUpperCase() + name.slice(1)
  const raw = rawMessage(e)
  if (NO_WALLET.test(raw)) {
    return `${Name} runs a K2 older than 0.45.0, which has no LLM tokens. Update that server to manage its tokens here.`
  }
  if (ROLE_REFUSED.test(raw)) {
    return `Your login on ${name} can't see its LLM tokens. Ask the server's owner for a Member login or higher.`
  }
  return `Couldn't load LLM tokens from ${name}: ${errorText(e)}`
}

// Entries are keyed by `scope.id`. The window's own server is always
// `primary`, whichever server the window is on, so a server switch must
// drop that entry (and make any in-flight load for it land nowhere):
// otherwise the page keeps showing the previous server's tokens under the
// new server — this computer's tokens on a remote, 0.45.0's bug.
const loadInflight = new Map<string, Promise<void>>()
const epochs = new Map<string, number>()
/** Bumped when a change this window sent is answered: a list read that
 *  began before then is older than the change and lands nowhere (a fresh
 *  read follows). Without it the read after a switch shared an earlier,
 *  still-running read and put the old default back on screen. */
const changeGens = new Map<string, number>()
/** The last list each server sent, before any pending switch is drawn on it. */
const served = new Map<string, LlmAccountsDoc>()
/** Switches sent and not yet answered, per server: tool → (token, seq). */
const pendingSwitches = new Map<string, Map<string, { id: string; seq: number }>>()
let switchSeq = 0
/** Mounted watchers of the window's server (`primary`). */
let primaryWatchers = 0

/** `daemon-cli`'s HostSwitchedError, matched by name (tests mock that
 *  module with the request helpers only). */
function hostSwitched(e: unknown): boolean {
  return e instanceof Error && e.name === 'HostSwitchedError'
}

function epochOf(key: string): number {
  return epochs.get(key) ?? 0
}

function changeGenOf(key: string): number {
  return changeGens.get(key) ?? 0
}

/** `doc` with `tool`'s server default moved to token `id`: the badge, the
 *  live token (a subscription only; an API token leaves the tool's own
 *  store as it was) and "since". */
export function withDefault(doc: LlmAccountsDoc, tool: string, id: string): LlmAccountsDoc {
  return {
    ...doc,
    tools: doc.tools.map((t) => {
      if (t.tool !== tool || t.activeId === id) return t
      const to = t.accounts.find((a) => a.id === id)
      if (!to) return t
      return {
        ...t,
        activeId: id,
        liveAccountId: to.kind === 'api_key' ? t.liveAccountId : id,
        switchedAt: Math.floor(Date.now() / 1000),
        accounts: t.accounts.map((a) => (a.active === (a.id === id) ? a : { ...a, active: a.id === id })),
      }
    }),
  }
}

/** The server's list with this window's unanswered switches drawn on it. */
function drawn(key: string, doc: LlmAccountsDoc): LlmAccountsDoc {
  const pending = pendingSwitches.get(key)
  if (!pending) return doc
  let out = doc
  for (const [tool, p] of pending) out = withDefault(out, tool, p.id)
  return out
}

/** Show the last served list again, with what is still pending. */
function redraw(key: string): void {
  const base = served.get(key)
  if (!base) return
  useLlmAccountsStore.setState((s) => {
    const entry = s.entries[key]
    if (!entry) return s
    return { entries: { ...s.entries, [key]: { ...entry, doc: drawn(key, base) } } }
  })
}

/** Forget one server's entry; an in-flight load for it lands nowhere. */
function dropEntry(key: string): void {
  epochs.set(key, epochOf(key) + 1)
  loadInflight.delete(key)
  served.delete(key)
  pendingSwitches.delete(key)
  useLlmAccountsStore.setState((s) => {
    if (!(key in s.entries)) return s
    const entries = { ...s.entries }
    delete entries[key]
    return { entries }
  })
}

interface LlmAccountsStore {
  entries: Record<string, AccountsEntry>
  /** `fresh`: the list changed after any read now in flight began (the
   *  daemon said so, or a change was answered), so read again once it
   *  lands rather than share its older answer. */
  load: (scope: ServerScope, opts?: { fresh?: boolean }) => Promise<void>
}

export const useLlmAccountsStore = create<LlmAccountsStore>((set, get) => ({
  entries: {},
  load: (scope, opts) => {
    const key = scope.id
    const running = loadInflight.get(key)
    if (running) return opts?.fresh ? running.then(() => get().load(scope)) : running
    const epoch = epochOf(key)
    const gen = changeGenOf(key)
    const p = (async () => {
      try {
        const doc = parseAccountsDoc(await daemonCliGet<unknown>(scope, 'llm/accounts/list'))
        if (epoch !== epochOf(key) || gen !== changeGenOf(key)) return
        served.set(key, doc)
        set((s) => ({ entries: { ...s.entries, [key]: { doc: drawn(key, doc), error: null, reason: null } } }))
      } catch (e) {
        // The window moved to another server mid-request: that server's
        // own load (after the switch) fills the entry. A read older than
        // an answered change: the fresh read after it fills it.
        if (epoch !== epochOf(key) || hostSwitched(e) || gen !== changeGenOf(key)) return
        // Keep the last list from THIS server (the entry is dropped on a
        // switch, so it is never another server's) and say what failed.
        set((s) => ({
          entries: {
            ...s.entries,
            [key]: { doc: s.entries[key]?.doc ?? null, error: loadErrorText(scope, e), reason: loadErrorReason(e) },
          },
        }))
      } finally {
        if (epoch === epochOf(key)) loadInflight.delete(key)
      }
    })()
    loadInflight.set(key, p)
    return p
  },
}))

/** The window switched servers (or minted a session on its server): drop
 *  the window's list, then load the new server's if the page is open. */
export function resetWindowLlmAccounts(): void {
  const scope = primaryScope()
  dropEntry(scope.id)
  if (primaryWatchers > 0) void useLlmAccountsStore.getState().load(scope)
}

onActiveHostChange(() => resetWindowLlmAccounts())

export function resetLlmAccountsForTests(): void {
  for (const key of new Set([...epochs.keys(), ...loadInflight.keys()])) epochs.set(key, epochOf(key) + 1)
  loadInflight.clear()
  served.clear()
  pendingSwitches.clear()
  primaryWatchers = 0
  useLlmAccountsStore.setState({ entries: {} })
}

/** Re-read at most this often on usage events: a usage event re-reads the
 *  whole account list (U28). */
export const USAGE_EVENT_DEBOUNCE_MS = 2_000

function pageHidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden'
}

/** Load now and on every `llm_accounts_changed` from that server (and,
 *  for the window's server, on every server switch), and when a usage
 *  reading changed (Members read usage from `account.usage`, so the list
 *  has to follow; debounced, skipped while the page is hidden). Returns
 *  the unsubscribe fn. */
export function watchLlmAccounts(scope: ServerScope): () => void {
  const primary = scope.isPrimary
  if (primary) primaryWatchers += 1
  void useLlmAccountsStore.getState().load(scope)
  // The daemon's news is newer than any read in flight: read after it.
  const off = onLlmAccountsChanged(scope, () => {
    void useLlmAccountsStore.getState().load(scope, { fresh: true })
  })
  let timer: ReturnType<typeof setTimeout> | null = null
  const offUsage = onSubscriptionUsageChanged(scope, () => {
    if (timer !== null || pageHidden()) return
    timer = setTimeout(() => {
      timer = null
      if (!pageHidden()) void useLlmAccountsStore.getState().load(scope)
    }, USAGE_EVENT_DEBOUNCE_MS)
  })
  let done = false
  return () => {
    if (done) return
    done = true
    if (primary) primaryWatchers = Math.max(0, primaryWatchers - 1)
    off()
    offUsage()
    if (timer !== null) clearTimeout(timer)
    timer = null
  }
}

/** A change was answered (or refused): any read that began before it is
 *  older than the server now; read the list again after it. */
function changed(scope: ServerScope): void {
  changeGens.set(scope.id, changeGenOf(scope.id) + 1)
  void useLlmAccountsStore.getState().load(scope, { fresh: true })
}

async function mutate<T>(scope: ServerScope, route: string, body: Record<string, unknown>): Promise<T> {
  try {
    return await daemonCliPost<T>(scope, `llm/accounts/${route}`, body)
  } finally {
    changed(scope)
  }
}

export function addLogin(
  scope: ServerScope,
  tool: string,
  label: string,
  mode: LoginMode,
): Promise<{ account: LlmAccount; login: LlmLogin }> {
  return mutate(scope, 'add', { tool, label, mode })
}

export function loginAgain(scope: ServerScope, id: string, mode: LoginMode): Promise<{ account: LlmAccount; login: LlmLogin }> {
  return mutate(scope, 'login', { id, mode })
}

export async function loginStatus(scope: ServerScope, loginId: string): Promise<LlmLogin> {
  const out = await daemonCliGet<{ login: LlmLogin }>(scope, 'llm/accounts/login/status', { loginId })
  return out.login
}

export function sendLoginInput(scope: ServerScope, loginId: string, text: string): Promise<{ ok: boolean }> {
  return daemonCliPost(scope, 'llm/accounts/login/input', { loginId, text })
}

export function cancelLogin(scope: ServerScope, loginId: string): Promise<{ login: LlmLogin }> {
  return mutate(scope, 'login/cancel', { loginId })
}

/** The token the daemon says is now the default (its answer's
 *  `account.id`, else `switch.to`); null on an answer without either. */
function answeredDefault(out: unknown): string | null {
  if (!out || typeof out !== 'object') return null
  const o = out as { account?: { id?: unknown }; switch?: { to?: unknown } }
  if (typeof o.account?.id === 'string') return o.account.id
  if (typeof o.switch?.to === 'string') return o.switch.to
  return null
}

/** Make token `id` its tool's server default. The list shows it at once;
 *  the daemon's answer confirms it (a refusal puts the old default back),
 *  then the list and the top-bar usage are read again. */
export async function switchLogin(scope: ServerScope, id: string): Promise<unknown> {
  const key = scope.id
  const entryDoc = useLlmAccountsStore.getState().entries[key]?.doc ?? null
  const tool = entryDoc?.tools.find((t) => t.accounts.some((a) => a.id === id))?.tool ?? null
  const seq = ++switchSeq
  if (tool && entryDoc) {
    if (!served.has(key)) served.set(key, entryDoc)
    const pending = pendingSwitches.get(key) ?? new Map<string, { id: string; seq: number }>()
    pending.set(tool, { id, seq })
    pendingSwitches.set(key, pending)
    redraw(key)
  }
  const settle = (): void => {
    if (!tool) return
    const pending = pendingSwitches.get(key)
    if (pending?.get(tool)?.seq === seq) pending.delete(tool)
    if (pending?.size === 0) pendingSwitches.delete(key)
  }
  try {
    const out = await daemonCliPost<unknown>(scope, 'llm/accounts/switch', { id })
    // Confirmed: the daemon's answer stands until the re-read lands.
    const base = served.get(key)
    if (tool && base) served.set(key, withDefault(base, tool, answeredDefault(out) ?? id))
    return out
  } finally {
    settle()
    redraw(key)
    changed(scope)
    // The top-bar chip reads that server's usage; read it again now (the
    // daemon also announces it, for every client).
    const usage = useSubscriptionUsageStore.getState()
    if (usage.entries[key]) void usage.load({ key, scope }, { fresh: true })
  }
}

export function switchToNext(scope: ServerScope, tool: string): Promise<unknown> {
  return mutate(scope, 'next', { tool })
}

export function renameLogin(scope: ServerScope, id: string, label: string): Promise<unknown> {
  return mutate(scope, 'rename', { id, label })
}

/** `clearDefault`: the person confirmed removing the server default API
 *  token, which leaves the tool with no default (Admin and above). */
export function removeLogin(scope: ServerScope, id: string, clearDefault = false): Promise<unknown> {
  return mutate(scope, 'remove', clearDefault ? { id, clearDefault: true } : { id })
}

/** No server default token for `tool` (an API token default only; Admin
 *  and above). A subscription left in the tool's own store becomes the
 *  default again. */
export function clearDefaultLogin(scope: ServerScope, tool: string): Promise<unknown> {
  return mutate(scope, 'clear-default', { tool })
}

export function refreshLogins(scope: ServerScope, usage: boolean): Promise<unknown> {
  return mutate(scope, 'refresh', { usage })
}

/** Add an API-key login. The key goes to the daemon once and is never
 *  returned; callers must drop it from their state right after. */
export function addApiKey(
  scope: ServerScope,
  tool: string,
  label: string,
  key: string,
): Promise<{ account: LlmAccount }> {
  return mutate(scope, 'add-key', { tool, label, key })
}

export type PinScope = 'workspace' | 'session'

/** Pin a workspace (scopeId = projects.id) or one chat (scopeId = its
 *  session key: the pinned chat's is the workspace id, a tab's is
 *  `tab-<terminalId>`) to a login. A chat also passes its conversation
 *  id when known, so the pick follows that conversation into any tab. */
export function pinLogin(
  scope: ServerScope,
  pinScope: PinScope,
  scopeId: string,
  tool: string,
  id: string,
  conversationId?: string | null,
): Promise<{ pin: LlmPin; note?: string }> {
  return mutate(scope, 'pin', {
    scope: pinScope,
    scopeId,
    tool,
    id,
    ...(conversationId ? { conversationId } : {}),
  })
}

/** Back to the pool for that scope + tool (a chat: its key and its
 *  conversation). */
export function unpinLogin(
  scope: ServerScope,
  pinScope: PinScope,
  scopeId: string,
  tool: string,
  conversationId?: string | null,
): Promise<{ unpinned: boolean }> {
  return mutate(scope, 'unpin', { scope: pinScope, scopeId, tool, ...(conversationId ? { conversationId } : {}) })
}

/** The pin for one scope + tool in a loaded doc, if any. */
export function pinFor(doc: LlmAccountsDoc | null, pinScope: PinScope, scopeId: string, tool: string): LlmPin | null {
  const t = doc?.tools.find((x) => x.tool === tool)
  return t?.pins.find((p) => p.scopeKind === pinScope && p.scopeId === scopeId) ?? null
}

/** The scope id of a chat pick that follows one conversation. */
export const CONVERSATION_PREFIX = 'conversation:'

/** A chat's token pick (the daemon's rule): the newer of its session
 *  key's pin and its conversation's pin; a tie goes to the session key. */
export function chatPinFor(
  doc: LlmAccountsDoc | null,
  sessionKey: string,
  conversationId: string | null | undefined,
  tool: string,
): LlmPin | null {
  const byKey = sessionKey ? pinFor(doc, 'session', sessionKey, tool) : null
  const conv = conversationId?.trim()
  const byConv = conv ? pinFor(doc, 'session', `${CONVERSATION_PREFIX}${conv}`, tool) : null
  if (byKey && byConv) return (byConv.createdAt ?? 0) > (byKey.createdAt ?? 0) ? byConv : byKey
  return byKey ?? byConv
}

/** Is this login pinned anywhere? */
export function isPinned(a: LlmAccount): boolean {
  return (a.pinnedTo?.length ?? 0) > 0
}

/** Can `a` be picked for a pin? A subscription that is in the live
 *  store (the server default) can't: it would be live in two places. */
export function pinBlockReason(tool: LlmTool, a: LlmAccount): string | null {
  if (a.kind === 'api_key') return null
  if (a.state !== 'signed_in') return 'Not signed in'
  const live = tool.liveAccountId ?? (tool.accounts.find((x) => x.id === tool.activeId)?.kind === 'api_key' ? null : tool.activeId)
  if (live === a.id) return 'This is the server default; pick Server default'
  return null
}

export const PINNED_SWITCH_REASON =
  "A workspace or chat uses this token, so it can't also be the server default. Set those back to Server default first."

export const CLAUDE_PIN_NOTE = "This chat's Claude history will stay with this subscription."

/** After picking a Claude subscription for one chat. */
export const CLAUDE_CHAT_PIN_NOTE = "K2 copies this chat's Claude history to this subscription when it restarts."

export const API_TOKEN_SUFFIX = 'billed per token'

function planTitle(plan: string | null | undefined): string {
  const p = (plan ?? '').trim()
  return p ? p.charAt(0).toUpperCase() + p.slice(1) : ''
}

/** What a person reads for a token: "Claude · Max · work" for a
 *  subscription, "API token · metered · billed per token" for a key. */
export function tokenName(toolDisplay: string, a: LlmAccount): string {
  if (a.kind === 'api_key') return `API token · ${a.label} · ${API_TOKEN_SUFFIX}`
  return [toolDisplay, planTitle(a.plan), a.label].filter(Boolean).join(' · ')
}

/** The token the server default resolves to right now (null = none). */
export function serverDefaultAccount(tool: LlmTool): LlmAccount | null {
  return tool.accounts.find((a) => a.id === tool.activeId) ?? null
}

/** How often an open login sheet re-reads `login/status`. */
export const LOGIN_POLL_MS = 1500

/** Is a sign-in running for this token (its slot may change any moment)? */
export function isSigningIn(doc: LlmAccountsDoc | null, a: LlmAccount): boolean {
  if (a.state === 'signing_in') return true
  return (doc?.logins ?? []).some((l) => l.accountId === a.id && !l.done)
}

/** When the token's usage was last read successfully (unix seconds):
 *  `usageCheckedAt`, else the reading's own `checkedAt`. */
export function usageCheckedAtSecs(a: LlmAccount): number | null {
  if (typeof a.usageCheckedAt === 'number' && a.usageCheckedAt > 0) return a.usageCheckedAt
  const t = a.usage?.checkedAt ? Date.parse(a.usage.checkedAt) : NaN
  return Number.isFinite(t) ? Math.floor(t / 1000) : null
}

/** "checked just now" / "checked 12 min ago" / "checked 3 h ago" /
 *  "checked 2 d ago" / "never checked". */
export function checkedAgoText(secs: number | null, nowMs: number = Date.now()): string {
  if (secs === null) return 'never checked'
  const d = Math.max(0, Math.floor(nowMs / 1000) - secs)
  if (d < 60) return 'checked just now'
  if (d < 3600) return `checked ${Math.floor(d / 60)} min ago`
  if (d < 86400) return `checked ${Math.floor(d / 3600)} h ago`
  return `checked ${Math.floor(d / 86400)} d ago`
}

/** A unix-seconds time as a person reads it ("Oct 14, 2:20 PM"). */
export function formatWhen(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })
}

/** Usage windows of the token's last reading that are used up and not
 *  reset yet, as warnings: "lumon is at 100% of its Weekly limit until …". */
export function exhaustedWarnings(a: LlmAccount, nowMs: number = Date.now()): string[] {
  const out: string[] = []
  for (const w of a.usage?.windows ?? []) {
    if (!(w.used >= 0.995)) continue
    const reset = w.resetsAt ? Date.parse(w.resetsAt) : NaN
    if (Number.isFinite(reset) && reset <= nowMs) continue
    const until = Number.isFinite(reset) ? ` until ${formatWhen(Math.floor(reset / 1000))}` : ''
    out.push(`${a.label} is at 100% of its ${w.label} limit${until}.`)
  }
  return out
}

/** The token "Use next token" moves to: the next signed-in subscription
 *  after the server default in pool order (the daemon's rule: no API
 *  tokens, nothing set for a workspace or chat, nothing signing in). */
export function nextCandidate(doc: LlmAccountsDoc | null, tool: LlmTool): LlmAccount | null {
  const list = tool.accounts
  if (list.length < 2) return null
  const idx = list.findIndex((a) => a.id === tool.activeId)
  const start = idx >= 0 ? idx : list.length - 1
  for (let step = 1; step < list.length; step++) {
    const a = list[(start + step) % list.length]
    if (a.id !== tool.activeId && a.kind !== 'api_key' && a.state === 'signed_in' && !isPinned(a) && !isSigningIn(doc, a)) {
      return a
    }
  }
  return null
}
