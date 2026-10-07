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
import type { ServerScope } from '@/kessel/server-scope'
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

export interface LlmAccountsDoc {
  tools: LlmTool[]
  logins: LlmLogin[]
  airgap: boolean
  switchNote: string
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

/** The daemon's `{"error":{"code","hint"}}` → a readable line. */
export function errorText(e: unknown): string {
  const raw = (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
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
  }
}

const loadInflight = new Map<string, Promise<void>>()

interface LlmAccountsStore {
  entries: Record<string, AccountsEntry>
  load: (scope: ServerScope) => Promise<void>
}

export const useLlmAccountsStore = create<LlmAccountsStore>((set) => ({
  entries: {},
  load: (scope) => {
    const running = loadInflight.get(scope.id)
    if (running) return running
    const p = (async () => {
      try {
        const doc = parseAccountsDoc(await daemonCliGet<unknown>(scope, 'llm/accounts/list'))
        set((s) => ({ entries: { ...s.entries, [scope.id]: { doc, error: null } } }))
      } catch (e) {
        set((s) => ({
          entries: { ...s.entries, [scope.id]: { doc: s.entries[scope.id]?.doc ?? null, error: errorText(e) } },
        }))
      } finally {
        loadInflight.delete(scope.id)
      }
    })()
    loadInflight.set(scope.id, p)
    return p
  },
}))

export function resetLlmAccountsForTests(): void {
  loadInflight.clear()
  useLlmAccountsStore.setState({ entries: {} })
}

/** Load now and on every `llm_accounts_changed` from that server.
 *  Returns the unsubscribe fn. */
export function watchLlmAccounts(scope: ServerScope): () => void {
  void useLlmAccountsStore.getState().load(scope)
  return onLlmAccountsChanged(scope, () => {
    void useLlmAccountsStore.getState().load(scope)
  })
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
