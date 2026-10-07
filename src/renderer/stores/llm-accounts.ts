// LLM login wallet (Settings → LLMs, `k2 llm accounts`), per server.
//
// One login per tool is active on a server; the others wait in the
// daemon's wallet (`~/.k2/llm-accounts`). The daemon owns every token and
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

export interface LlmAccount {
  id: string
  tool: string
  label: string
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
  activeId: string | null
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

/** Tools with a wallet in this version. */
export const WALLET_TOOLS = ['claude', 'codex', 'grok'] as const

export const STATE_LABELS: Record<string, string> = {
  signed_in: 'Signed in',
  needs_login: 'Needs login',
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
  const tools = asArray<Record<string, unknown>>(o.tools).map((t) => ({
    tool: String(t.tool ?? ''),
    display: String(t.display ?? t.tool ?? ''),
    supported: t.supported === true,
    activeId: typeof t.activeId === 'string' ? t.activeId : null,
    loginMethod: (t.loginMethod === 'live_swap' || t.loginMethod === 'temp_home' ? t.loginMethod : null) as LlmTool['loginMethod'],
    accounts: asArray<LlmAccount>(t.accounts),
  }))
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

/** How often an open login sheet re-reads `login/status`. */
export const LOGIN_POLL_MS = 1500
