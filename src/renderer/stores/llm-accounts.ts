// LLM tokens (Settings → LLMs, `k2 llm tokens`), per server. People see
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
import { onLlmAccountsChanged } from '@/stores/session-events'
import type { HarnessUsage } from '@/lib/subscription-usage'

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
  url: string | null
  code: string | null
  error: string | null
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
}

export interface AccountsEntry {
  doc: LlmAccountsDoc | null
  error: string | null
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

/** A load failure as a person reads it, naming the server (hosted web
 *  is one server, and its primary scope's label reads "This computer"). */
export function loadErrorText(scope: ServerScope, e: unknown): string {
  const name = isWebClient() ? 'This server' : scope.label
  const raw = rawMessage(e)
  if (NO_WALLET.test(raw)) {
    return `${name} runs a K2 older than 0.45.0, which has no LLM tokens. Update that server to manage its tokens here.`
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

/** Forget one server's entry; an in-flight load for it lands nowhere. */
function dropEntry(key: string): void {
  epochs.set(key, epochOf(key) + 1)
  loadInflight.delete(key)
  useLlmAccountsStore.setState((s) => {
    if (!(key in s.entries)) return s
    const entries = { ...s.entries }
    delete entries[key]
    return { entries }
  })
}

interface LlmAccountsStore {
  entries: Record<string, AccountsEntry>
  load: (scope: ServerScope) => Promise<void>
}

export const useLlmAccountsStore = create<LlmAccountsStore>((set) => ({
  entries: {},
  load: (scope) => {
    const key = scope.id
    const running = loadInflight.get(key)
    if (running) return running
    const epoch = epochOf(key)
    const p = (async () => {
      try {
        const doc = parseAccountsDoc(await daemonCliGet<unknown>(scope, 'llm/accounts/list'))
        if (epoch !== epochOf(key)) return
        set((s) => ({ entries: { ...s.entries, [key]: { doc, error: null } } }))
      } catch (e) {
        // The window moved to another server mid-request: that server's
        // own load (after the switch) fills the entry.
        if (epoch !== epochOf(key) || hostSwitched(e)) return
        // Keep the last list from THIS server (the entry is dropped on a
        // switch, so it is never another server's) and say what failed.
        set((s) => ({
          entries: { ...s.entries, [key]: { doc: s.entries[key]?.doc ?? null, error: loadErrorText(scope, e) } },
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
  primaryWatchers = 0
  useLlmAccountsStore.setState({ entries: {} })
}

/** Load now and on every `llm_accounts_changed` from that server (and,
 *  for the window's server, on every server switch). Returns the
 *  unsubscribe fn. */
export function watchLlmAccounts(scope: ServerScope): () => void {
  const primary = scope.isPrimary
  if (primary) primaryWatchers += 1
  void useLlmAccountsStore.getState().load(scope)
  const off = onLlmAccountsChanged(scope, () => {
    void useLlmAccountsStore.getState().load(scope)
  })
  let done = false
  return () => {
    if (done) return
    done = true
    if (primary) primaryWatchers = Math.max(0, primaryWatchers - 1)
    off()
  }
}

async function mutate<T>(scope: ServerScope, route: string, body: Record<string, unknown>): Promise<T> {
  const out = await daemonCliPost<T>(scope, `llm/accounts/${route}`, body)
  void useLlmAccountsStore.getState().load(scope)
  return out
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

export function switchLogin(scope: ServerScope, id: string): Promise<unknown> {
  return mutate(scope, 'switch', { id })
}

export function switchToNext(scope: ServerScope, tool: string): Promise<unknown> {
  return mutate(scope, 'next', { tool })
}

export function renameLogin(scope: ServerScope, id: string, label: string): Promise<unknown> {
  return mutate(scope, 'rename', { id, label })
}

export function removeLogin(scope: ServerScope, id: string): Promise<unknown> {
  return mutate(scope, 'remove', { id })
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
  "This token is set for a workspace or chat, so it can't also be the server default (a subscription can't be live in two places). Set those back to Server default first."

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
