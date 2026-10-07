// Settings → LLMs, right column: "Logins" — the LLM login wallet.
//
// One login per tool is active on the server (the tool's normal store), so
// every session uses it and conversations never split. The other logins
// wait in the daemon's wallet. Switching affects every session on the
// server; running sessions pick up the new login. K2 never switches on its
// own: "Switch to next login" is a click.

import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { primaryScope } from '@/kessel/server-scope'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  errorText,
  removeLogin,
  renameLogin,
  STATE_LABELS,
  switchLogin,
  switchToNext,
  useLlmAccountsStore,
  watchLlmAccounts,
  type LlmAccount,
  type LlmTool,
} from '@/stores/llm-accounts'
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
  'One login per tool is active on this server. Switching affects every session on this server; running sessions pick up the new login.'

const STATE_COLORS: Record<string, string> = {
  signed_in: '#22c55e',
  needs_login: '#ef4444',
  signing_in: '#eab308',
  not_set_up: '#6b7280',
  unknown: '#6b7280',
}

function UsageMeter({ account }: { account: LlmAccount }): React.JSX.Element | null {
  const windows = account.usage?.windows ?? []
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
  const signedIn = account.state === 'signed_in'
  const needsLogin = account.state === 'needs_login' || account.state === 'not_set_up'
  const who = [account.email, account.org].filter(Boolean).join(' · ')

  const run = useCallback(
    (p: Promise<unknown>) => {
      p.catch((e: unknown) => onError(errorText(e)))
    },
    [onError],
  )

  const remove = useCallback(async () => {
    const ok = await confirm({
      title: `Remove ${account.label}?`,
      message: `K2 moves this ${tool.display} login to the wallet's trash on the server. Log it in again later to use it.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (ok) run(removeLogin(scope, account.id))
  }, [confirm, account, tool.display, run, scope])

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
            <span className="text-xs text-[var(--color-text-secondary)] truncate">{account.label}</span>
          )}
          {account.active && (
            <span className="text-[8px] uppercase tracking-wider font-semibold px-1 py-px border border-[var(--color-accent)] text-[var(--color-accent)]">
              Active
            </span>
          )}
          <span className="text-[10px] text-[var(--color-text-muted)]">
            {STATE_LABELS[account.state] ?? account.state}
          </span>
        </div>
        <div className="flex items-center gap-2 flex-shrink-0 text-[10px]">
          {!account.active && signedIn && (
            <button type="button" className="text-[var(--color-accent)]" onClick={() => run(switchLogin(scope, account.id))}>
              Make active
            </button>
          )}
          {!account.active && needsLogin && (
            <button type="button" className="text-[var(--color-accent)]" onClick={() => onLoginAgain(account)}>
              Log in again
            </button>
          )}
          <button type="button" className="text-[var(--color-text-muted)]" onClick={() => setRenaming(true)}>
            Rename
          </button>
          <button
            type="button"
            className="text-[var(--color-text-muted)] disabled:opacity-40"
            disabled={account.active}
            title={account.active ? 'Switch to another login first' : undefined}
            onClick={() => void remove()}
          >
            Remove
          </button>
        </div>
      </div>
      {(who || account.plan) && (
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5 truncate">
          {[who, account.plan].filter(Boolean).join(' · ')}
        </div>
      )}
      {account.detail && account.state !== 'signed_in' && (
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{account.detail}</div>
      )}
      <UsageMeter account={account} />
    </div>
  )
}

export function AgentAccountsColumn(): React.JSX.Element {
  const scope = primaryScope()
  const entry = useLlmAccountsStore((s) => s.entries[scope.id])
  const [sheet, setSheet] = useState<LoginSheetStart | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => watchLlmAccounts(scope), [scope])

  const doc = entry?.doc ?? null
  const byTool = new Map((doc?.tools ?? []).map((t) => [t.tool, t]))

  return (
    <div className="w-full" data-settings-id="agents.accounts">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Logins</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed" data-testid="llm-switch-note">
        {SWITCH_NOTE}
      </p>
      {doc?.airgap && (
        <p className="text-[10px] text-[var(--color-text-muted)] mb-3">
          Air-gap mode is on: signing in and refreshing logins are off. Switching between saved logins still works.
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
          const signedIn = (tool?.accounts ?? []).filter((a) => a.state === 'signed_in')
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
                  <button
                    type="button"
                    className="text-[10px] text-[var(--color-accent)] disabled:opacity-40"
                    disabled={doc?.airgap === true}
                    onClick={() => setSheet({ kind: 'add', tool: row.id, toolLabel: row.label })}
                  >
                    + Add login
                  </button>
                ) : (
                  <span className="text-[10px] text-[var(--color-text-muted)]">Not available yet</span>
                )}
              </div>
              {supported && tool && tool.accounts.length === 0 && (
                <div className="px-3 pb-2 text-[10px] text-[var(--color-text-muted)]">
                  Not signed in on this server.
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
              {supported && tool && signedIn.length >= 2 && (
                <div className="px-3 py-2 border-t border-[var(--color-border)] flex items-center justify-between gap-2">
                  <span className="text-[10px] text-[var(--color-text-muted)]">
                    Use this when a session hits its limit. K2 never switches on its own.
                  </span>
                  <button
                    type="button"
                    className="text-[10px] text-[var(--color-accent)] flex-shrink-0"
                    onClick={() => {
                      switchToNext(scope, row.id).catch((e: unknown) => setError(errorText(e)))
                    }}
                  >
                    Switch to next login
                  </button>
                </div>
              )}
            </div>
          )
        })}
      </div>
      {sheet && <LlmLoginSheet scope={scope} start={sheet} onClose={() => setSheet(null)} />}
    </div>
  )
}
