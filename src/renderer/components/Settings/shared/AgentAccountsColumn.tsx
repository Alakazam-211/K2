// Settings → Language Models → Subs / Tokens — subscriptions and API tokens (its own page
// from 0.45.7, LlmTokensSection; before, the right column of Settings → LLMs).
// (Internally: the LLM login wallet; routes and types keep those names.)
//
// The page follows Settings → Workspaces / Agents and Admin Access: a title
// with one plain line, the page's one primary action (Add a token), then a
// card per provider with a row per token (name and facts left, the row's
// main action and its "…" menu on one right edge). Loading, failed and
// empty use the shared load-state card; a change says what it did in the
// provider's card; anything that removes or moves the default asks first,
// in the same dialog frame as Admin Access.
//
// The server default ("the pool" internally): one token per tool is the
// server default (the tool's normal store), so every chat on Server
// default uses it and conversations never split. The other tokens wait in
// the daemon's store. Changing the server default affects every chat on
// Server default; running sessions pick up the new token. A workspace or
// chat can use its own token (workspace settings → Agent → LLM tokens, or
// the chat header's Token picker); those keep their token. API tokens are
// billed per token. K2 never switches on its own: "Use next token" is a
// click.

import React from 'react'
import { useCallback, useEffect, useId, useMemo, useState } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { isWebClient } from '@/lib/is-web'
import { formatResetsIn, isChipWindow, limitState, limitStateText, sourceText } from '@/lib/subscription-usage'
import { USAGE_CACHE_POLL_MS } from '@/stores/subscription-usage'
import { findLimitsLogin, type LimitsLogin } from '@/lib/usage-limits'
import { useUsageLimitsStore, watchUsageLimits } from '@/stores/usage-limits'
import { LimitBar } from '@/components/Usage/LimitBar'
import {
  checkedAgoText,
  clearDefaultLogin,
  errorText,
  exhaustedWarnings,
  formatWhen,
  isPinned,
  isSigningIn,
  nextCandidate,
  PINNED_SWITCH_REASON,
  refreshLogins,
  removeLogin,
  renameLogin,
  serverInSentence,
  STATE_LABELS,
  switchLogin,
  tokenName,
  usageCheckedAtSecs,
  useLlmAccountsStore,
  watchLlmAccounts,
  type LlmAccount,
  type LlmAccountsDoc,
  type LlmTool,
} from '@/stores/llm-accounts'
import { LlmLoginSheet, type LoginSheetResult, type LoginSheetStart } from './LlmLoginSheet'
import { SIGN_IN_TOOLS, useLlmSignInRequestStore } from '@/stores/llm-sign-in-request'
import { Button, Callout, Input, OverflowMenu, Tag, type OverflowMenuItem } from '@/components/ui'
import { SETTING_ROW_HINT, SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { AccessSheet, ConfirmSheet, DIALOG_FIELD, Field } from '../sections/AccessDialogs'
import { StatusDot, type HealthTone } from '../sections/ConnectionsParts'

/** The big 7 rows, in the page's order, with their icon names. */
export const LOGIN_TOOL_ROWS: Array<{ id: string; label: string; agentIcon: string }> = [
  { id: 'claude', label: 'Claude', agentIcon: 'Claude' },
  { id: 'codex', label: 'Codex', agentIcon: 'Codex' },
  { id: 'grok', label: 'Grok', agentIcon: 'Grok' },
  { id: 'gemini', label: 'Gemini', agentIcon: 'Gemini' },
  { id: 'cursor', label: 'Cursor Agent', agentIcon: 'Cursor Agent' },
  { id: 'hermes', label: 'Hermes', agentIcon: 'Hermes' },
  { id: 'pi', label: 'Pi', agentIcon: 'Pi' },
]

/** The page's line under its title: what it's for. */
export const PAGE_INTRO = 'The subscriptions and API tokens your agents sign in with.'

/** How the server default works, in the page's words. */
export const SWITCH_NOTE =
  'Each provider has one server default: every chat set to Server default uses it, and running chats pick up a change.'

/** Said wherever the default moves: who keeps their own token. */
export const KEEP_OWN_NOTE = 'Workspaces and chats that picked their own token keep it.'

/** A Member login can't change the server default (Admin and above). */
export const ADMIN_SWITCH_REASON = 'Only an Admin or the Owner can change the server default.'

export const ACTIVE_REMOVE_REASON = 'This is the server default. Make another token the default first.'

/** On the server default's menu: why it has no "Sign in again". */
export const ACTIVE_RELOGIN_HINT = 'To sign it in again, make another token the server default first.'

export const SIGNING_IN_SWITCH_REASON = 'This token is signing in. Finish or cancel that first.'

/** What "no server default" means for `tool` (said in the confirm). A
 *  subscription still signed in to the tool itself becomes the default
 *  again; otherwise chats on Server default get no token from K2. */
export function noDefaultText(tool: Pick<LlmTool, 'display' | 'activeId' | 'liveAccountId' | 'accounts'>): string {
  const live = tool.liveAccountId && tool.liveAccountId !== tool.activeId
    ? tool.accounts.find((a) => a.id === tool.liveAccountId) ?? null
    : null
  return live
    ? `“${live.label}”, still signed in to ${tool.display} itself, becomes the server default again.`
    : `${tool.display} then has no server default: chats set to Server default get no token from K2.`
}

/** The confirm before a server-default switch: from → to, plus a
 *  warning (never a refusal) when the target's last reading is used up. */
export function switchConfirmText(
  tool: LlmTool,
  from: LlmAccount | null,
  to: LlmAccount,
  nowMs: number = Date.now(),
): { title: string; message: string; warnings: string[] } {
  const toName = tokenName(tool.display, to)
  const fromName = from ? tokenName(tool.display, from) : 'no token'
  const warnings = exhaustedWarnings(to, nowMs)
  const lines = [
    `From: ${fromName}`,
    `To: ${toName}`,
    '',
    `Every chat set to Server default on this server moves to “${to.label}”, and running chats pick it up. ${KEEP_OWN_NOTE}`,
  ]
  if (warnings.length > 0) lines.push('', ...warnings.map((w) => `Warning: ${w}`))
  return { title: `Make “${to.label}” the ${tool.display} server default?`, message: lines.join('\n'), warnings }
}

const STATE_TONE: Record<string, HealthTone> = {
  signed_in: 'ok',
  needs_login: 'error',
  signing_in: 'warn',
}

/** "Max subscription · alex@example.com · Example Studio": what a token is. */
function kindLine(a: LlmAccount): string {
  if (a.kind === 'api_key') return 'API token · billed per token'
  const plan = (a.plan ?? '').trim()
  const kind = plan ? `${plan.charAt(0).toUpperCase()}${plan.slice(1)} subscription` : 'Subscription'
  return [kind, a.email, a.org].filter(Boolean).join(' · ')
}

/** Usage v2 (Q4, P8): each token's subscription windows as T3-style bars
 *  (all of them, B30: model-scoped weeks too), with the pace mark, amber /
 *  red bands, "reset — re-checking", how old the reading is and why there
 *  are no numbers. The reading is the daemon's `/cli/usage/limits` row for
 *  this token when this login may read it (Owner, Admin), else the
 *  wallet's own last reading. "Check usage now" is in the row's menu. */
function UsageMeter({
  account,
  limits,
  refreshing,
  refreshError,
}: {
  account: LlmAccount
  limits: LimitsLogin | null
  refreshing: boolean
  refreshError: string | null
}): React.JSX.Element | null {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), USAGE_CACHE_POLL_MS)
    return () => clearInterval(id)
  }, [])
  const usage = limits ?? account.usage
  // API tokens: the row already says "billed per token".
  if (!usage || account.kind === 'api_key') return null
  const windows = usage.windows ?? []
  // Each token's own last reading, with its age: a failed check keeps the
  // old time and says so, so old numbers never read as just checked.
  const checkedSecs = limits ? rfc3339Secs(limits.checkedAt) : usageCheckedAtSecs(account)
  const age = checkedAgoText(checkedSecs, now)
  const state = limitState(usage)
  const why = state === 'ok' || state === 'probe_failed' || state === 'no_window' ? null : limitStateText(usage)
  const live = sourceText(usage.source)
  const nextProbe = usage.nextProbeAt ? Date.parse(usage.nextProbeAt) : NaN
  return (
    <div className="mt-2" data-testid={`llm-usage-${account.id}`}>
      {windows.length > 0 && (
        <div className="flex flex-wrap gap-x-4 gap-y-1.5">
          {windows.map((w) => (
            <div key={w.id ?? w.label} className="w-44">
              <LimitBar window={w} nowMs={now} dense drivesChip={isChipWindow(usage.chipWindow, w)} labelTestId={`llm-usage-window-${account.id}-${w.label}`} />
            </div>
          ))}
        </div>
      )}
      {why && (
        <p
          className={`type-hint mt-1 ${state === 'expired' ? 'text-[var(--color-status-warn-amber-soft)]' : 'text-[var(--color-text-secondary)]'}`}
          data-testid={`llm-usage-state-${account.id}`}
        >
          {why}
        </p>
      )}
      <p className="type-hint text-[var(--color-text-secondary)] mt-1" data-testid={`llm-usage-age-${account.id}`}>
        {windows.length === 0 && !why && usage.status ? `${usage.status} · ` : ''}
        {refreshing ? 'Checking usage…' : `Usage ${age}`}
        {live ? ` · ${live}` : ''}
        {Number.isFinite(nextProbe) && nextProbe > now ? ` · next check ${formatResetsIn(usage.nextProbeAt ?? '', now)}` : ''}
        {usage.error && (
          <span className="text-[var(--color-status-warn-amber-soft)]" data-testid={`llm-usage-error-${account.id}`}>
            {' '}· last check failed ({usage.error})
          </span>
        )}
      </p>
      {refreshError && (
        <p className="type-hint text-[var(--color-status-error-soft)] mt-1" role="alert">
          Couldn’t check usage. The server said: {refreshError.replace(/[.\s]+$/, '')}.
        </p>
      )}
    </div>
  )
}

function rfc3339Secs(iso: string): number | null {
  const t = iso ? Date.parse(iso) : NaN
  return Number.isFinite(t) ? Math.floor(t / 1000) : null
}

function pinLabel(a: LlmAccount): string {
  return (a.pinnedTo ?? [])
    .map((p) => (p.scopeKind === 'workspace' ? p.label : `chat ${p.label}`))
    .join(', ')
}

/** What one row may do, and why not. */
interface RowActions {
  onMakeDefault: (a: LlmAccount) => void
  onLoginAgain: (a: LlmAccount) => void
  onRename: (a: LlmAccount) => void
  onRemove: (a: LlmAccount) => void
  onNoDefault: (a: LlmAccount) => void
}

function AccountRow({
  account,
  tool,
  doc,
  limits,
  onRefreshLimits,
  actions,
}: {
  account: LlmAccount
  tool: LlmTool
  doc: LlmAccountsDoc
  /** This token's `/cli/usage/limits` row (Owner/Admin), else null. */
  limits: LimitsLogin | null
  onRefreshLimits?: (accountId: string) => Promise<void>
  actions: RowActions
}): React.JSX.Element {
  const [refreshing, setRefreshing] = useState(false)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const apiKey = account.kind === 'api_key'
  const signedIn = account.state === 'signed_in'
  const needsLogin = !apiKey && (account.state === 'needs_login' || account.state === 'not_set_up')
  // A subscription set for a workspace or chat can't be live in two
  // places (rotating tokens): it can't become the server default, be
  // removed or be signed in again until those go back to Server default.
  const pinnedBlock = !apiKey && isPinned(account)
  // A sign-in rewrites the token's slot: it can't become the default
  // until that finishes.
  const signingIn = isSigningIn(doc, account)
  const switchBlocked = !doc.canChangeDefault
    ? (doc.changeDefaultReason ?? ADMIN_SWITCH_REASON)
    : pinnedBlock
      ? PINNED_SWITCH_REASON
      : signingIn
        ? SIGNING_IN_SWITCH_REASON
        : undefined
  const pinnedTo = pinLabel(account)
  // The server default API token may be removed (or just stop being the
  // default) after a confirm: that leaves the tool with no default.
  // Admin and above, like any default change. A subscription default
  // can't: it sits in the tool's own store.
  const clearsDefault = account.active && apiKey
  const removeBlocked = account.active
    ? clearsDefault
      ? (doc.canChangeDefault ? undefined : (doc.changeDefaultReason ?? ADMIN_SWITCH_REASON))
      : ACTIVE_REMOVE_REASON
    : pinnedBlock
      ? PINNED_SWITCH_REASON
      : undefined
  // Any subscription that isn't the server default can be signed in
  // again (same flow as `k2 llm tokens login`).
  const canLoginAgain = !account.active && !apiKey && (signedIn || needsLogin)

  const refresh = (): void => {
    if (!onRefreshLimits) return
    setRefreshing(true)
    setRefreshError(null)
    onRefreshLimits(account.id)
      .catch((e: unknown) => setRefreshError(errorText(e)))
      .finally(() => setRefreshing(false))
  }

  // The row's one button: what this token most likely needs next.
  const primary: React.ReactNode = !account.active && signedIn ? (
    <Button
      disabled={switchBlocked !== undefined}
      title={switchBlocked ?? `Make “${account.label}” the ${tool.display} server default`}
      onClick={() => actions.onMakeDefault(account)}
    >
      Make default
    </Button>
  ) : needsLogin && !account.active ? (
    <Button
      disabled={pinnedBlock}
      title={pinnedBlock ? PINNED_SWITCH_REASON : `Sign “${account.label}” in again`}
      onClick={() => actions.onLoginAgain(account)}
    >
      Sign in again
    </Button>
  ) : null

  const items: OverflowMenuItem[] = [
    { id: 'rename', label: 'Rename…', onSelect: () => actions.onRename(account) },
  ]
  if (!apiKey && !(needsLogin && !account.active)) {
    items.push({
      id: 'relogin',
      label: 'Sign in again…',
      disabled: !canLoginAgain || pinnedBlock,
      hint: account.active ? ACTIVE_RELOGIN_HINT : pinnedBlock ? PINNED_SWITCH_REASON : undefined,
      onSelect: () => actions.onLoginAgain(account),
    })
  }
  if (onRefreshLimits && !apiKey) {
    items.push({ id: 'usage', label: 'Check usage now', disabled: refreshing, onSelect: refresh })
  }
  if (clearsDefault) {
    items.push({
      id: 'no-default',
      label: 'Stop using as default…',
      disabled: !doc.canChangeDefault,
      hint: doc.canChangeDefault ? `${tool.display} is left with no server default.` : (doc.changeDefaultReason ?? ADMIN_SWITCH_REASON),
      onSelect: () => actions.onNoDefault(account),
    })
  }
  items.push({
    id: 'remove',
    label: 'Remove…',
    danger: true,
    group: true,
    disabled: removeBlocked !== undefined,
    hint: removeBlocked,
    onSelect: () => actions.onRemove(account),
  })

  const facts: string[] = []
  if (account.active && tool.switchedAt !== null) facts.push(`Server default since ${formatWhen(tool.switchedAt)}`)
  if (pinnedTo) facts.push(`Used by ${pinnedTo}`)
  if (account.inUse) facts.push('In use by a chat right now')

  return (
    <div className="px-4 py-3 transition-colors hover:bg-[var(--color-bg-hover)]" data-testid={`llm-account-${account.id}`}>
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3">
        <span title={STATE_LABELS[account.state] ?? account.state} className="flex">
          <StatusDot tone={STATE_TONE[account.state] ?? 'neutral'} />
        </span>
        <div className="flex items-center gap-2 min-w-0 flex-wrap">
          <span className="type-name text-[var(--color-text-primary)] truncate" data-testid={`llm-name-${account.id}`}>
            {account.label}
          </span>
          {account.active && <Tag tone="accent">Server default</Tag>}
          {signedIn ? null : (
            <Tag tone={needsLogin ? 'warn' : 'muted'} data-testid={`llm-state-${account.id}`}>
              {STATE_LABELS[account.state] ?? account.state}
            </Tag>
          )}
          {account.addedOutsideK2 && (
            <Tag
              tone="muted"
              title="Someone signed in outside K2, for example with /login in a terminal, and K2 saved that sign-in as this token. Keep it, or remove it."
              data-testid={`llm-outside-${account.id}`}
            >
              Added outside K2
            </Tag>
          )}
        </div>
        <div className="flex items-center gap-2 pl-3">
          {primary}
          <OverflowMenu label={`More actions for ${account.label}`} items={items} />
        </div>
        <div className="col-start-2 min-w-0">
          <p className={`${SETTING_ROW_HINT} truncate`} data-testid={`llm-kind-${account.id}`}>
            {kindLine(account)}
          </p>
          {facts.length > 0 && (
            <p className={SETTING_ROW_HINT} data-testid={`llm-facts-${account.id}`}>
              {facts.join(' · ')}
            </p>
          )}
          {account.detail && account.state !== 'signed_in' && <p className={SETTING_ROW_HINT}>{account.detail}</p>}
          <UsageMeter account={account} limits={limits} refreshing={refreshing} refreshError={refreshError} />
        </div>
      </div>
    </div>
  )
}

/** A change in one provider's card: what happened, or what failed. */
type Note =
  | { tool: string; kind: 'ok'; text: string }
  /** `next` defaults to "Nothing changed; try again."; `detail` is the
   *  server's reason, when there is one. */
  | { tool: string; kind: 'error'; what: string; next?: string; detail?: string }

function NoteRow({ note, onDismiss }: { note: Note; onDismiss: () => void }): React.JSX.Element {
  if (note.kind === 'error') {
    return (
      <div role="alert" className="px-4 py-2.5 flex items-start justify-between gap-4" data-testid="llm-row-error">
        <div className={`${SETTING_ROW_HINT} mt-0`}>
          <p className="text-[var(--color-status-error-soft)]">
            {note.what} {note.next ?? 'Nothing changed; try again.'}
          </p>
          {note.detail ? <p>The server said: {note.detail.trim().replace(/[.\s]+$/, '')}.</p> : null}
        </div>
        <Button variant="quiet" onClick={onDismiss}>
          Dismiss
        </Button>
      </div>
    )
  }
  return (
    <div role="status" className="px-4 py-2.5 flex items-center justify-between gap-4" data-testid="llm-notice">
      <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-status-ok-soft)]">
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
          <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
        </svg>
        {note.text}
      </span>
      <Button variant="quiet" onClick={onDismiss}>
        Dismiss
      </Button>
    </div>
  )
}

/** Rename a token: one field, Save. */
function RenameDialog({
  account,
  tool,
  onClose,
  onSave,
}: {
  account: LlmAccount
  tool: LlmTool
  onClose: () => void
  /** Saves; resolves to an error message, or null. */
  onSave: (label: string) => Promise<string | null>
}): React.JSX.Element {
  const [label, setLabel] = useState(account.label)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const id = useId()
  const next = label.trim()
  const submit = async (): Promise<void> => {
    if (!next || busy) return
    if (next === account.label) return onClose()
    setBusy(true)
    const err = await onSave(next)
    setBusy(false)
    if (err) setError(err)
    else onClose()
  }
  return (
    <AccessSheet title={`Rename “${account.label}”`} onClose={onClose} busy={busy} testId="llm-rename-dialog">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <Field label="Name" htmlFor={id} hint={`Shown here and in every ${tool.display} Token menu on this server.`}>
          <Input id={id} autoFocus className={DIALOG_FIELD} aria-label="New label" value={label} onChange={(e) => setLabel(e.target.value)} />
        </Field>
        {error ? (
          <Callout tone="error" role="alert" className="type-hint space-y-0.5">
            <p className="font-medium">Couldn’t rename it. The name is unchanged; try again.</p>
            <p>The server said: {error.replace(/[.\s]+$/, '')}.</p>
          </Callout>
        ) : null}
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="accent" size="md" disabled={!next || busy}>
            {busy ? 'Saving…' : 'Save name'}
          </Button>
        </div>
      </form>
    </AccessSheet>
  )
}

/** A question before a change: the dialog it opens. */
type Ask =
  | { kind: 'switch'; tool: LlmTool; to: LlmAccount }
  | { kind: 'remove'; tool: LlmTool; account: LlmAccount }
  | { kind: 'no-default'; tool: LlmTool; account: LlmAccount }
  | { kind: 'rename'; tool: LlmTool; account: LlmAccount }

export function AgentAccountsColumn(): React.JSX.Element {
  // The window's server, resolved at call time: every read and every
  // gesture on this page goes to that server's daemon, never to this
  // computer's unless this computer is the window's server. The store
  // drops the list and reloads on a server switch; this subscription
  // re-renders the label and clears this server's leftover sheet/notes.
  const scope = primaryScope()
  const hostKey = useConnectHostStore((s) => activeHostKey(s.activeHost))
  const entry = useLlmAccountsStore((s) => s.entries[scope.id])
  const [sheet, setSheet] = useState<LoginSheetStart | null>(null)
  const [ask, setAsk] = useState<Ask | null>(null)
  const [note, setNote] = useState<Note | null>(null)

  useEffect(() => watchLlmAccounts(scope), [scope])
  // Usage v2: every login's limits for the bars (Owner/Admin; a Member's
  // 403 leaves each row on its wallet reading). One watcher per server.
  const limitsTarget = useMemo(() => ({ key: scope.id, scope }), [scope])
  useEffect(() => watchUsageLimits(limitsTarget), [limitsTarget])
  const limitsDoc = useUsageLimitsStore((s) => s.entries[scope.id]?.doc ?? null)
  const refreshLimits = useCallback(
    (accountId: string) => useUsageLimitsStore.getState().refresh(limitsTarget, { accountId }),
    [limitsTarget],
  )
  useEffect(() => {
    setSheet(null)
    setAsk(null)
    setNote(null)
  }, [hostKey])
  // A sign-in asked for from outside Settings (the setup checklist): once
  // the list is here, open the sheet for that tool, or for the first of
  // Claude, Codex and Grok that can sign in. Declared after the reset
  // above, so a request that arrives with the page's first render wins.
  const signInRequest = useLlmSignInRequestStore((s) => s.pending)
  const loadedDoc = entry?.doc ?? null
  useEffect(() => {
    if (!signInRequest || !loadedDoc) return
    useLlmSignInRequestStore.getState().take()
    if (loadedDoc.airgap) return
    const canSignIn = (tool: string): boolean => {
      const t = loadedDoc.tools.find((x) => x.tool === tool)
      return t?.supported === true && t.subscription === true
    }
    const pick = [signInRequest, ...SIGN_IN_TOOLS].find((r) => canSignIn(r.tool))
    if (pick) setSheet({ kind: 'add', tool: pick.tool, toolLabel: pick.toolLabel })
  }, [signInRequest, loadedDoc])
  // Opening the page re-reads every token's usage on that server (the
  // daemon runs it at most once a minute); the list reloads when the
  // daemon says it changed. A failure here is not the page's error: each
  // row says how old its numbers are.
  useEffect(() => {
    refreshLogins(scope, true).catch((e: unknown) => {
      console.debug('[llm-tokens] usage refresh on open:', errorText(e))
    })
  }, [scope, hostKey])

  /** Run a change; say what it did, or what failed, in that provider's card. */
  const change = useCallback(
    async (tool: string, p: Promise<unknown>, ok: string, what: string): Promise<string | null> => {
      try {
        await p
        setNote({ tool, kind: 'ok', text: ok })
        return null
      } catch (e) {
        const detail = errorText(e)
        setNote({ tool, kind: 'error', what, detail })
        return detail
      }
    },
    [],
  )

  const askSwitchToNext = useCallback((tool: LlmTool, docNow: LlmAccountsDoc) => {
    const to = nextCandidate(docNow, tool)
    if (!to) {
      setNote({
        tool: tool.tool,
        kind: 'error',
        what: `There’s no other signed-in ${tool.display} subscription to move to.`,
        next: 'Sign one in again, or add one with Add a token.',
      })
      return
    }
    // The exact token the dialog names (not the daemon's own "next").
    setAsk({ kind: 'switch', tool, to })
  }, [])

  const actionsFor = (tool: LlmTool): RowActions => ({
    onMakeDefault: (a) => setAsk({ kind: 'switch', tool, to: a }),
    onLoginAgain: (a) => setSheet({ kind: 'relogin', tool: tool.tool, toolLabel: tool.display, accountId: a.id, label: a.label }),
    onRename: (a) => setAsk({ kind: 'rename', tool, account: a }),
    onRemove: (a) => setAsk({ kind: 'remove', tool, account: a }),
    onNoDefault: (a) => setAsk({ kind: 'no-default', tool, account: a }),
  })

  const onSheetDone = useCallback((r: LoginSheetResult) => {
    const display = LOGIN_TOOL_ROWS.find((t) => t.id === r.tool)?.label ?? r.tool
    setNote({
      tool: r.tool,
      kind: 'ok',
      text:
        r.method === 'key'
          ? `${display} API token “${r.label}” was added.`
          : r.method === 'relogin'
            ? `“${r.label}” is signed in again.`
            : `${display} “${r.label}” is signed in and saved.`,
    })
  }, [])

  const doc = entry?.doc ?? null
  const byTool = new Map((doc?.tools ?? []).map((t) => [t.tool, t]))
  const supportedRows = LOGIN_TOOL_ROWS.filter((r) => byTool.get(r.id)?.supported === true)
  const notYet = LOGIN_TOOL_ROWS.filter((r) => byTool.get(r.id)?.supported !== true).map((r) => r.label)
  const total = supportedRows.reduce((n, r) => n + (byTool.get(r.id)?.accounts.length ?? 0), 0)
  const server = serverInSentence(scope)

  const addButton = (
    <Button variant="accent" onClick={() => setSheet({ kind: 'new' })} disabled={!doc || supportedRows.length === 0}>
      Add a token
    </Button>
  )

  const body = (() => {
    if (!entry) {
      return (
        <SettingsLoadState kind="loading" title="Loading tokens…" testId="llm-accounts-loading">
          Reading the subscriptions and API tokens saved on {server}.
        </SettingsLoadState>
      )
    }
    if (!doc) {
      return (
        <SettingsLoadState
          kind="error"
          title={`Couldn’t load the tokens on ${server}`}
          detail={entry.reason}
          action={<Button onClick={() => void useLlmAccountsStore.getState().load(scope)}>Try again</Button>}
          testId="llm-accounts-error"
        >
          {entry.reason ? 'Try again in a moment. Your tokens and the server default haven’t changed.' : entry.error}
        </SettingsLoadState>
      )
    }
    if (supportedRows.length > 0 && total === 0) {
      return (
        <SettingsLoadState
          kind="empty"
          title="No tokens on this server yet"
          action={addButton}
          testId="llm-accounts-empty"
        >
          Add a subscription or an API token for {listWords(supportedRows.map((r) => r.label))} so your agents can sign in.
        </SettingsLoadState>
      )
    }
    return (
      <div className="space-y-6" data-settings-id="agents.add-login">
        {supportedRows.map((row) => {
          const tool = byTool.get(row.id)!
          const canSignIn = tool.subscription === true
          const canKey = tool.apiKeys === true
          // "Use next token" cycles the server default's subscriptions only:
          // no API tokens, nothing set for a workspace or chat.
          const cyclable = tool.accounts.filter((a) => a.state === 'signed_in' && a.kind !== 'api_key' && !isPinned(a))
          const activeAcc = tool.accounts.find((a) => a.id === tool.activeId)
          const keptLive =
            activeAcc?.kind === 'api_key' && tool.liveAccountId ? tool.accounts.find((a) => a.id === tool.liveAccountId) : undefined
          const hint = [
            canSignIn && canKey ? 'Sign in with a subscription, or paste an API token.' : canSignIn ? 'Sign in with a subscription.' : 'API tokens only.',
            tool.accounts.length > 0 && !activeAcc ? 'No server default yet: chats set to Server default get no token from K2.' : null,
            keptLive ? `“${keptLive.label}” is still signed in to ${row.label} itself.` : null,
          ]
            .filter(Boolean)
            .join(' ')
          const toolNote = note?.tool === row.id ? note : null
          return (
            <SettingsGroup key={row.id} title={row.label} hint={hint} card testId={`llm-tool-${row.id}`}>
              {toolNote ? <NoteRow note={toolNote} onDismiss={() => setNote(null)} /> : null}
              {tool.accounts.length === 0 ? (
                <SettingRow
                  label={`No ${row.label} token yet`}
                  hint={canSignIn ? `Sign in with a ${row.label} subscription, or paste an API token.` : `Paste a ${row.label} API token.`}
                  testId={`llm-empty-${row.id}`}
                >
                  <Button aria-label={`Add a ${row.label} token`} onClick={() => setSheet({ kind: 'new', tool: row.id })}>
                    Add
                  </Button>
                </SettingRow>
              ) : null}
              {tool.accounts.map((a) => (
                <AccountRow
                  key={a.id}
                  account={a}
                  tool={tool}
                  doc={doc}
                  limits={findLimitsLogin(limitsDoc, a.id)}
                  onRefreshLimits={limitsDoc ? refreshLimits : undefined}
                  actions={actionsFor(tool)}
                />
              ))}
              {cyclable.length >= 2 ? (
                <SettingRow
                  label="Hit a usage limit?"
                  hint={`Move the server default to the next signed-in ${row.label} subscription. K2 never switches on its own.`}
                >
                  <Button
                    disabled={!doc.canChangeDefault}
                    title={doc.canChangeDefault ? `Switch ${row.label} to its next signed-in token` : (doc.changeDefaultReason ?? ADMIN_SWITCH_REASON)}
                    onClick={() => askSwitchToNext(tool, doc)}
                  >
                    Use next token
                  </Button>
                </SettingRow>
              ) : null}
            </SettingsGroup>
          )
        })}
        {notYet.length > 0 ? (
          <p className={SETTING_ROW_HINT} data-testid="llm-not-yet">
            {listWords(notYet)} can’t use saved tokens yet.
          </p>
        ) : null}
      </div>
    )
  })()

  return (
    <div className="w-full space-y-6 pb-6" data-settings-id="agents.accounts">
      <header className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <h2 className="type-title text-[var(--color-text-primary)]">Subs / Tokens</h2>
          <p className={SETTING_ROW_HINT} data-testid="llm-switch-note">
            {PAGE_INTRO} {SWITCH_NOTE}
          </p>
          {/* Hosted web is one server and its primary scope reads "This
              computer", so the line is desktop-only. */}
          {!isWebClient() && (
            <p className={SETTING_ROW_HINT} data-testid="llm-server">
              Server: {scope.label}
            </p>
          )}
        </div>
        {doc && total > 0 ? <div className="flex-shrink-0 pt-0.5">{addButton}</div> : null}
      </header>

      {doc && !doc.canChangeDefault && (
        <Callout tone="info" className="type-hint">
          <p data-testid="llm-default-admin-only">{doc.changeDefaultReason ?? ADMIN_SWITCH_REASON}</p>
        </Callout>
      )}
      {doc?.airgap && (
        <Callout tone="info" className="type-hint" data-testid="llm-airgap">
          Air-gap mode is on: you can’t add subscriptions or refresh sign-ins. Switching between saved tokens still works.
        </Callout>
      )}
      {(doc?.keychainRepairs ?? []).map((r) => (
        <Callout key={`${r.service}\u0000${r.account}`} tone="warn" role="alert" className="type-hint space-y-1.5" data-testid="llm-keychain-repair">
          <p className="leading-relaxed">
            macOS asks for your password every time Claude reads its saved sign-in “{r.service}” ({r.holds}), because an
            earlier K2 update changed who may read it. Run this once in Terminal on this Mac and enter your Mac password:
          </p>
          <code className="block type-hint font-mono select-all break-all text-[var(--color-text-primary)]">{r.command}</code>
        </Callout>
      ))}
      {doc && entry?.error ? (
        <SettingsLoadState
          kind="error"
          title="Couldn’t refresh this list"
          detail={entry.reason}
          action={<Button onClick={() => void useLlmAccountsStore.getState().load(scope)}>Try again</Button>}
          testId="llm-accounts-error"
        >
          {entry.reason ? 'It shows what K2 last read from this server.' : entry.error}
        </SettingsLoadState>
      ) : null}

      {body}

      {sheet && <LlmLoginSheet scope={scope} start={sheet} onClose={() => setSheet(null)} onDone={onSheetDone} />}
      {ask?.kind === 'switch' ? (
        (() => {
          const from = ask.tool.accounts.find((a) => a.id === ask.tool.activeId) ?? null
          const { title, message } = switchConfirmText(ask.tool, from, ask.to)
          return (
            <ConfirmSheet
              title={title}
              message={<span className="whitespace-pre-line">{message}</span>}
              confirmLabel="Make default"
              testId="llm-switch-confirm"
              onCancel={() => setAsk(null)}
              onConfirm={() => {
                const { tool, to } = ask
                setAsk(null)
                void change(tool.tool, switchLogin(scope, to.id), `“${to.label}” is now the ${tool.display} server default.`, 'Couldn’t change the server default.')
              }}
            />
          )
        })()
      ) : ask?.kind === 'remove' ? (
        <ConfirmSheet
          title={`Remove “${ask.account.label}”?`}
          message={removeText(ask.tool, ask.account)}
          confirmLabel="Remove"
          destructive
          testId="llm-remove-confirm"
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            const { tool, account } = ask
            setAsk(null)
            const clears = account.active && account.kind === 'api_key'
            void change(tool.tool, removeLogin(scope, account.id, clears), `“${account.label}” was removed from this server.`, `Couldn’t remove “${account.label}”.`)
          }}
        />
      ) : ask?.kind === 'no-default' ? (
        <ConfirmSheet
          title={`Stop using “${ask.account.label}” as the default?`}
          message={`“${ask.account.label}” stays saved. ${noDefaultText(ask.tool)}`}
          confirmLabel="Stop using as default"
          testId="llm-no-default-confirm"
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            const { tool, account } = ask
            setAsk(null)
            void change(tool.tool, clearDefaultLogin(scope, tool.tool), `“${account.label}” is no longer the ${tool.display} server default.`, 'Couldn’t change the server default.')
          }}
        />
      ) : ask?.kind === 'rename' ? (
        <RenameDialog
          account={ask.account}
          tool={ask.tool}
          onClose={() => setAsk(null)}
          onSave={async (label) => {
            const { tool, account } = ask
            try {
              await renameLogin(scope, account.id, label)
              setNote({ tool: tool.tool, kind: 'ok', text: `“${account.label}” is now called “${label}”.` })
              return null
            } catch (e) {
              return errorText(e)
            }
          }}
        />
      ) : null}
    </div>
  )
}

/** What removing a token does, in the confirm. */
function removeText(tool: LlmTool, a: LlmAccount): string {
  if (a.active && a.kind === 'api_key') {
    return `It’s the ${tool.display} server default. ${noDefaultText(tool)} To use it again, paste the token again.`
  }
  return a.kind === 'api_key'
    ? `K2 stops using this ${tool.display} API token on this server. To use it again, paste the token again.`
    : `K2 stops using this ${tool.display} subscription on this server. You can sign it in again later with Add a token.`
}

/** "Claude, Codex and Grok". */
function listWords(words: string[]): string {
  if (words.length <= 1) return words.join('')
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`
}

