// Pins: which LLM login a workspace (all its agents) or one session uses.
//
// "Pool (default)" = the tool's active login on this server (Settings →
// LLMs). A specific login pins the workspace / session to it: new sessions
// run on that login; a resumed conversation keeps the login it started on.
// A sign-in that is the pool's live login can't be pinned (one login is
// never live in two places). API keys are billed per token.
//
// The scope is passed in by the caller (workspace settings: the window's
// primary server; chat header: the room's server).

import React from 'react'
import { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { SettingDropdown } from '../controls/SettingControls'
import {
  CLAUDE_PIN_NOTE,
  errorText,
  pinBlockReason,
  pinFor,
  pinLogin,
  unpinLogin,
  useLlmAccountsStore,
  watchLlmAccounts,
  type LlmAccountsDoc,
  type LlmTool,
  type PinScope,
} from '@/stores/llm-accounts'

export const POOL_VALUE = '__pool__'
export const POOL_LABEL = 'Pool (default)'
export const SESSION_PIN_NOTE = 'Applies to new chats; a resumed chat keeps the login it started on.'

/** Chat provider id → wallet tool (null = no logins for that harness). */
export function toolForProvider(provider: string | null | undefined): string | null {
  switch ((provider ?? '').toLowerCase()) {
    case 'claude':
      return 'claude'
    case 'codex':
      return 'codex'
    case 'grok':
      return 'grok'
    case 'gemini':
      return 'gemini'
    default:
      return null
  }
}

function options(tool: LlmTool): { value: string; label: string; disabled?: boolean }[] {
  const opts: { value: string; label: string; disabled?: boolean }[] = [{ value: POOL_VALUE, label: POOL_LABEL }]
  for (const a of tool.accounts) {
    const block = pinBlockReason(tool, a)
    const suffix = a.kind === 'api_key' ? ' · billed per token' : ''
    opts.push({
      value: a.id,
      label: block ? `${a.label}${suffix} — ${block}` : `${a.label}${suffix}`,
      disabled: block !== null,
    })
  }
  return opts
}

function useDoc(scope: ServerScope): LlmAccountsDoc | null {
  const entry = useLlmAccountsStore((s) => s.entries[scope.id])
  useEffect(() => watchLlmAccounts(scope), [scope])
  return entry?.doc ?? null
}

function PinRow({
  scope,
  doc,
  tool,
  pinScope,
  scopeId,
  compact,
}: {
  scope: ServerScope
  doc: LlmAccountsDoc
  tool: LlmTool
  pinScope: PinScope
  scopeId: string
  compact?: boolean
}): React.JSX.Element {
  const pin = pinFor(doc, pinScope, scopeId, tool.tool)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const pinnedAcc = pin ? tool.accounts.find((a) => a.id === pin.accountId) : undefined
  const choose = async (value: string): Promise<void> => {
    setError(null)
    setNote(null)
    try {
      if (value === POOL_VALUE) {
        if (pin) await unpinLogin(scope, pinScope, scopeId, tool.tool)
      } else {
        await pinLogin(scope, pinScope, scopeId, tool.tool, value)
        const acc = tool.accounts.find((a) => a.id === value)
        if (tool.tool === 'claude' && acc?.kind !== 'api_key') setNote(CLAUDE_PIN_NOTE)
      }
    } catch (e) {
      setError(errorText(e))
    }
  }
  return (
    <div className={compact ? '' : 'py-1.5'} data-testid={`llm-pin-${pinScope}-${tool.tool}`}>
      <div className="flex items-center justify-between gap-3">
        {!compact && <span className="text-xs text-[var(--color-text-secondary)]">{tool.display}</span>}
        <SettingDropdown
          value={pin ? pin.accountId : POOL_VALUE}
          options={options(tool)}
          onChange={(v) => choose(v)}
          ariaLabel={`${tool.display} login`}
        />
      </div>
      {pinnedAcc?.kind === 'api_key' && (
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Billed per token.</p>
      )}
      {note && <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{note}</p>}
      {error && (
        <p className="text-[10px] text-red-400 mt-0.5" role="alert">
          {error}
        </p>
      )}
    </div>
  )
}

/** Workspace settings → Agent → "LLM logins": one row per tool. */
export function WorkspaceLlmLogins({ scope, projectId }: { scope: ServerScope; projectId: string }): React.JSX.Element {
  const doc = useDoc(scope)
  const tools = (doc?.tools ?? []).filter((t) => t.supported)
  return (
    <div data-settings-id="projects.llm-logins" data-testid="workspace-llm-logins">
      <p className="text-[10px] text-[var(--color-text-muted)] mb-1 leading-relaxed">
        Pool uses the login that is active on this server (Settings → LLMs). Pick a login to run this
        workspace&apos;s agents and sidecars on it instead. Resumed chats keep the login they started on.
      </p>
      {!doc && <p className="text-[10px] text-[var(--color-text-muted)]">Loading logins…</p>}
      {doc &&
        tools.map((t) => (
          <PinRow key={t.tool} scope={scope} doc={doc} tool={t} pinScope="workspace" scopeId={projectId} />
        ))}
    </div>
  )
}

/** Chat header "Login" chip for the pinned chat (session key = the
 *  workspace id). Hidden when the tool has no saved logins. */
export function SessionLoginPicker({
  scope,
  projectId,
  provider,
}: {
  scope: ServerScope
  projectId: string
  provider: string | null | undefined
}): React.JSX.Element | null {
  const doc = useDoc(scope)
  const [open, setOpen] = useState(false)
  const toolId = toolForProvider(provider)
  const tool = doc?.tools.find((t) => t.tool === toolId && t.supported)
  if (!doc || !tool || tool.accounts.length === 0) return null
  const pin = pinFor(doc, 'session', projectId, tool.tool)
  const pinned = pin ? tool.accounts.find((a) => a.id === pin.accountId) : undefined
  return (
    <div className="relative self-center" data-testid="chat-login-picker">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-label={`${tool.display} login for this chat`}
        aria-expanded={open}
        className="inline-flex items-center gap-1 px-2 py-0.5 text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] no-drag cursor-pointer"
      >
        Login: {pinned ? pinned.label : 'Pool'}
      </button>
      {open && (
        <div className="absolute right-0 top-full mt-1 z-30 w-[34ch] bg-[var(--color-bg)] border border-[var(--color-border)] shadow-2xl p-2 space-y-1">
          <PinRow scope={scope} doc={doc} tool={tool} pinScope="session" scopeId={projectId} compact />
          {pin && (
            <div className="flex flex-col gap-0.5 pt-1 border-t border-[var(--color-border)]">
              <span className="text-[10px] text-[var(--color-text-muted)]">Hit a limit on this login?</span>
              <button
                type="button"
                className="text-left text-[10px] text-[var(--color-accent)]"
                onClick={() => {
                  void unpinLogin(scope, 'session', projectId, tool.tool).catch(() => {})
                  setOpen(false)
                }}
              >
                Unpin and use the pool
              </button>
              <span className="text-[10px] text-[var(--color-text-muted)]">
                Or switch this pin to another login above.
              </span>
            </div>
          )}
          <p className="text-[10px] text-[var(--color-text-muted)]">{SESSION_PIN_NOTE}</p>
        </div>
      )}
    </div>
  )
}
