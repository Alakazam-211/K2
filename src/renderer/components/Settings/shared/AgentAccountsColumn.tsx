// Settings → LLMs, right column: "Tokens" — subscriptions and API tokens.
// (Internally: the LLM login wallet; routes and types keep those names.)
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
import { useCallback, useEffect, useState } from 'react'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { primaryScope } from '@/kessel/server-scope'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  addApiKey,
  errorText,
  isPinned,
  PINNED_SWITCH_REASON,
  removeLogin,
  renameLogin,
  STATE_LABELS,
  switchLogin,
  switchToNext,
  tokenName,
  useLlmAccountsStore,
  watchLlmAccounts,
  type LlmAccount,
  type LlmTool,
} from '@/stores/llm-accounts'
import type { ServerScope } from '@/kessel/server-scope'
import { LlmLoginSheet, type LoginSheetStart } from './LlmLoginSheet'

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

export const SWITCH_NOTE =
  'One token per tool is the server default. Changing it affects every chat that uses Server default on this server; running sessions pick up the new token. Workspaces and chats set to their own token keep it.'

export const ACTIVE_REMOVE_REASON = 'This is the server default; make another token the server default first'

const STATE_COLORS: Record<string, string> = {
  signed_in: '#22c55e',
  needs_login: '#ef4444',
  signing_in: '#eab308',
  not_set_up: '#6b7280',
  unknown: '#6b7280',
}

function UsageMeter({ account }: { account: LlmAccount }): React.JSX.Element | null {
  const windows = account.usage?.windows ?? []
  // API tokens: the row name already says "billed per token".
  if (windows.length === 0) return null
  return (
    <div className="flex gap-3 mt-1" data-testid={`llm-usage-${account.id}`}>
      {windows.slice(0, 2).map((w) => {
        const pct = Math.max(0, Math.min(100, Math.round(w.used * 100)))
        return (
          <div key={w.label} className="min-w-0 w-24">
            <div className="text-[9px] text-[var(--color-text-muted)] truncate">
              {w.label} {pct}%
            </div>
            <div className="h-0.5 bg-[var(--color-border)]">
              <div
                className="h-0.5"
                style={{ width: `${pct}%`, backgroundColor: pct >= 90 ? '#ef4444' : 'var(--color-accent)' }}
              />
            </div>
          </div>
        )
      })}
    </div>
  )
}

function pinLabel(a: LlmAccount): string {
  return (a.pinnedTo ?? [])
    .map((p) => (p.scopeKind === 'workspace' ? p.label : `chat ${p.label}`))
    .join(', ')
}

function AccountRow({
  account,
  tool,
  onLoginAgain,
  onError,
}: {
  account: LlmAccount
  tool: LlmTool
  onLoginAgain: (a: LlmAccount) => void
  onError: (msg: string) => void
}): React.JSX.Element {
  const scope = primaryScope()
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const [renaming, setRenaming] = useState(false)
  const [newLabel, setNewLabel] = useState(account.label)
  const apiKey = account.kind === 'api_key'
  const signedIn = account.state === 'signed_in'
  const needsLogin = !apiKey && (account.state === 'needs_login' || account.state === 'not_set_up')
  // A subscription set for a workspace or chat can't be live in two
  // places (rotating tokens): it can't become the server default, be
  // removed or be signed in again until those go back to Server default.
  const pinnedBlock = !apiKey && isPinned(account)
  const who = [account.email, account.org].filter(Boolean).join(' · ')
  const pinnedTo = pinLabel(account)

  const run = useCallback(
    (p: Promise<unknown>) => {
      p.catch((e: unknown) => onError(errorText(e)))
    },
    [onError],
  )

  const remove = useCallback(async () => {
    const ok = await confirm({
      title: `Remove ${account.label}?`,
      message: apiKey
        ? `K2 moves this ${tool.display} API token to the trash on the server.`
        : `K2 moves this ${tool.display} subscription to the trash on the server. Add it again later to use it.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (ok) run(removeLogin(scope, account.id))
  }, [confirm, account, apiKey, tool.display, run, scope])

  const removeBlocked = account.active ? ACTIVE_REMOVE_REASON : pinnedBlock ? PINNED_SWITCH_REASON : undefined

  return (
    <div className="px-3 py-2 border-t border-[var(--color-border)]" data-testid={`llm-account-${account.id}`}>
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0 flex items-center gap-1.5">
          <span
            className="w-1.5 h-1.5 flex-shrink-0"
            style={{ backgroundColor: STATE_COLORS[account.state] ?? STATE_COLORS.unknown }}
          />
          {renaming ? (
            <input
              autoFocus
              value={newLabel}
              aria-label="New label"
              onChange={(e) => setNewLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && newLabel.trim()) {
                  setRenaming(false)
                  run(renameLogin(scope, account.id, newLabel.trim()))
                } else if (e.key === 'Escape') {
                  setRenaming(false)
                  setNewLabel(account.label)
                }
              }}
              className="w-28 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] px-1 text-xs text-[var(--color-text-primary)] outline-none"
            />
          ) : (
            <span className="text-xs text-[var(--color-text-secondary)] truncate" data-testid={`llm-name-${account.id}`}>
              {tokenName(tool.display, account)}
            </span>
          )}
          {account.active && (
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1 py-px border border-[var(--color-accent)] text-[var(--color-accent)] flex-shrink-0">
              Server default
            </span>
          )}
          <span className="text-[10px] text-[var(--color-text-muted)]">
            {STATE_LABELS[account.state] ?? account.state}
          </span>
        </div>
        <div className="flex items-center gap-2 flex-shrink-0 text-[10px]">
          {!account.active && signedIn && (
            <button
              type="button"
              className="text-[var(--color-accent)] disabled:opacity-40"
              disabled={pinnedBlock}
              title={pinnedBlock ? PINNED_SWITCH_REASON : undefined}
              onClick={() => run(switchLogin(scope, account.id))}
            >
              Use this token
            </button>
          )}
          {!account.active && needsLogin && (
            <button
              type="button"
              className="text-[var(--color-accent)] disabled:opacity-40"
              disabled={pinnedBlock}
              title={pinnedBlock ? PINNED_SWITCH_REASON : undefined}
              onClick={() => onLoginAgain(account)}
            >
              Sign in again
            </button>
          )}
          <button type="button" className="text-[var(--color-text-muted)]" onClick={() => setRenaming(true)}>
            Rename
          </button>
          <button
            type="button"
            className="text-[var(--color-text-muted)] disabled:opacity-40"
            disabled={removeBlocked !== undefined}
            title={removeBlocked}
            onClick={() => void remove()}
          >
            Remove
          </button>
        </div>
      </div>
      {who && <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">{who}</div>}
      {pinnedTo && (
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate" data-testid={`llm-pinned-${account.id}`}>
          Used by: {pinnedTo}
        </div>
      )}
      {account.inUse && (
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5" data-testid={`llm-inuse-${account.id}`}>
          In use by a chat right now
        </div>
      )}
      {account.detail && account.state !== 'signed_in' && (
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{account.detail}</div>
      )}
      <UsageMeter account={account} />
    </div>
  )
}

/** "+ Add API token": label + key (password field). The key is sent once
 *  and cleared from state right after; the daemon never returns it. */
function ApiKeyDialog({
  scope,
  tool,
  toolLabel,
  onClose,
}: {
  scope: ServerScope
  tool: string
  toolLabel: string
  onClose: () => void
}): React.JSX.Element {
  const [label, setLabel] = useState('')
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = useCallback(async () => {
    const sentKey = key.trim()
    setKey('')
    setBusy(true)
    setError(null)
    try {
      await addApiKey(scope, tool, label.trim(), sentKey)
      onClose()
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [scope, tool, label, key, onClose])
  return (
    <div
      className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/50"
      role="dialog"
      aria-label={`Add ${toolLabel} API token`}
      data-testid="llm-apikey-dialog"
    >
      <div className="w-[min(420px,92vw)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] p-4 space-y-3">
        <h3 className="text-sm text-[var(--color-text-primary)]">Add {toolLabel} API token</h3>
        <label className="block text-[10px] text-[var(--color-text-muted)]">
          Label
          <input
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            aria-label="API token label"
            className="mt-1 w-full bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1 text-xs text-[var(--color-text-primary)] outline-none"
          />
        </label>
        <label className="block text-[10px] text-[var(--color-text-muted)]">
          API token
          <input
            type="password"
            autoComplete="off"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            aria-label="API token"
            className="mt-1 w-full bg-[var(--color-bg)] border border-[var(--color-border)] px-2 py-1 text-xs text-[var(--color-text-primary)] outline-none"
          />
        </label>
        <p className="text-[10px] text-[var(--color-text-muted)]">
          Billed per token by the provider. The key stays on the server and is never shown again.
        </p>
        {error && (
          <p className="text-[10px] text-red-400" role="alert">
            {error}
          </p>
        )}
        <div className="flex justify-end gap-2 text-xs">
          <button type="button" className="text-[var(--color-text-muted)]" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="text-[var(--color-accent)] disabled:opacity-40"
            disabled={busy || !label.trim() || !key.trim()}
            onClick={() => void submit()}
          >
            Add API token
          </button>
        </div>
      </div>
    </div>
  )
}

export function AgentAccountsColumn(): React.JSX.Element {
  const scope = primaryScope()
  const entry = useLlmAccountsStore((s) => s.entries[scope.id])
  const [sheet, setSheet] = useState<LoginSheetStart | null>(null)
  const [keyDialog, setKeyDialog] = useState<{ tool: string; toolLabel: string } | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => watchLlmAccounts(scope), [scope])

  const doc = entry?.doc ?? null
  const byTool = new Map((doc?.tools ?? []).map((t) => [t.tool, t]))

  return (
    <div className="w-full" data-settings-id="agents.accounts">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Tokens</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed" data-testid="llm-switch-note">
        {SWITCH_NOTE}
      </p>
      {doc?.airgap && (
        <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
          Air-gap mode is on: adding subscriptions and refreshing tokens are off. Switching between saved tokens still works.
        </p>
      )}
      {entry?.error && !doc && (
        <p className="text-[10px] text-red-400 mb-3" data-testid="llm-accounts-error">
          {entry.error}
        </p>
      )}
      {error && (
        <p className="text-[10px] text-red-400 mb-3" role="alert">
          {error}
        </p>
      )}
      <div className="border border-[var(--color-border)]" data-settings-id="agents.add-login">
        {LOGIN_TOOL_ROWS.map((row, i) => {
          const tool = byTool.get(row.id)
          const supported = tool?.supported === true
          const canSignIn = supported && tool?.subscription === true
          const canKey = supported && tool?.apiKeys === true
          // "Use next token" cycles the server default's subscriptions only:
          // no API tokens, nothing set for a workspace or chat.
          const cyclable = (tool?.accounts ?? []).filter(
            (a) => a.state === 'signed_in' && a.kind !== 'api_key' && !isPinned(a),
          )
          const activeAcc = tool?.accounts.find((a) => a.id === tool.activeId)
          const keptLive =
            activeAcc?.kind === 'api_key' && tool?.liveAccountId
              ? tool.accounts.find((a) => a.id === tool.liveAccountId)
              : undefined
          return (
            <div
              key={row.id}
              data-testid={`llm-tool-${row.id}`}
              className={i === LOGIN_TOOL_ROWS.length - 1 ? '' : 'border-b border-[var(--color-border)]'}
            >
              <div className="flex items-center justify-between gap-3 px-3 py-2.5">
                <div className="flex items-center gap-2 min-w-0">
                  <AgentIcon agent={row.agentIcon} size={14} />
                  <div className="text-xs text-[var(--color-text-secondary)] truncate">{row.label}</div>
                </div>
                {supported ? (
                  <div className="flex items-center gap-3 flex-shrink-0">
                    {canSignIn && (
                      <button
                        type="button"
                        className="text-[10px] text-[var(--color-accent)] disabled:opacity-40"
                        disabled={doc?.airgap === true}
                        onClick={() => setSheet({ kind: 'add', tool: row.id, toolLabel: row.label })}
                      >
                        + Add subscription
                      </button>
                    )}
                    {canKey && (
                      <button
                        type="button"
                        className="text-[10px] text-[var(--color-accent)]"
                        onClick={() => setKeyDialog({ tool: row.id, toolLabel: row.label })}
                      >
                        + Add API token
                      </button>
                    )}
                  </div>
                ) : (
                  <span className="text-[10px] text-[var(--color-text-muted)]">Not available yet</span>
                )}
              </div>
              {supported && tool && tool.accounts.length === 0 && (
                <div className="px-3 pb-2 text-[10px] text-[var(--color-text-muted)]">
                  {canSignIn ? 'No subscription on this server yet.' : 'No API token on this server yet.'}
                </div>
              )}
              {keptLive && (
                <div className="px-3 pb-2 text-[10px] text-[var(--color-text-muted)]" data-testid={`llm-kept-live-${row.id}`}>
                  Subscription still signed in for {row.label}: {tokenName(row.label, keptLive)}
                </div>
              )}
              {supported &&
                tool?.accounts.map((a) => (
                  <AccountRow
                    key={a.id}
                    account={a}
                    tool={tool}
                    onError={setError}
                    onLoginAgain={(acc) =>
                      setSheet({ kind: 'relogin', tool: row.id, toolLabel: row.label, accountId: acc.id, label: acc.label })
                    }
                  />
                ))}
              {supported && tool && cyclable.length >= 2 && (
                <div className="px-3 py-2 border-t border-[var(--color-border)] flex items-center justify-between gap-2">
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    Use this when a session hits its limit. Applies to chats on Server default. K2 never switches on its own.
                  </span>
                  <button
                    type="button"
                    className="text-[10px] text-[var(--color-accent)] flex-shrink-0"
                    onClick={() => {
                      switchToNext(scope, row.id).catch((e: unknown) => setError(errorText(e)))
                    }}
                  >
                    Use next token
                  </button>
                </div>
              )}
            </div>
          )
        })}
      </div>
      {sheet && <LlmLoginSheet scope={scope} start={sheet} onClose={() => setSheet(null)} />}
      {keyDialog && (
        <ApiKeyDialog
          scope={scope}
          tool={keyDialog.tool}
          toolLabel={keyDialog.toolLabel}
          onClose={() => setKeyDialog(null)}
        />
      )}
    </div>
  )
}
