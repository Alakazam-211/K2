// Token pickers: which LLM token a workspace (all its agents) or one chat
// uses. People read "token": a subscription (a CLI sign-in) or an API
// token. Internally these are pins on wallet accounts (routes unchanged).
//
// The closed control says only "Token"; the open menu marks the one in
// use. "Server default — <name>" = the tool's server default token
// (Settings → LLMs; "the pool" internally) and follows its rotation. A
// chat whose workspace has its own token defaults to that instead
// ("Workspace default — <name>"). Picking a subscription or API token
// for a workspace: its new chats run on it; a resumed chat keeps the
// token it started on. Picking one for a chat (the pinned chat, or any
// agent chat tab: its header or right-click → Token): the chat runs on
// it from its next start and at every revival (reopened, resumed,
// restored after a restart); the pick is keyed on the chat's session
// key and its conversation id. The subscription that is the server
// default can't also be picked here (one sign-in is never live in two
// places). API tokens are billed per token.
//
// The scope is passed in by the caller (workspace settings: the window's
// primary server; chat header / tab menu: the room's server).

import React from 'react'
import { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { SettingDropdown } from '../controls/SettingControls'
import {
  CLAUDE_CHAT_PIN_NOTE,
  CLAUDE_PIN_NOTE,
  chatPinFor,
  errorText,
  pinBlockReason,
  pinFor,
  pinLogin,
  serverDefaultAccount,
  tokenName,
  unpinLogin,
  useLlmAccountsStore,
  watchLlmAccounts,
  type LlmAccountsDoc,
  type LlmPin,
  type LlmTool,
  type PinScope,
} from '@/stores/llm-accounts'
import type { ContextMenuItemDef } from '@/stores/context-menu'

export const DEFAULT_VALUE = '__pool__'
export const SERVER_DEFAULT_LABEL = 'Server default'
export const WORKSPACE_DEFAULT_LABEL = 'Workspace default'
export const TOKEN_LABEL = 'Token'
export const IN_USE = 'In use'
export const SUBSCRIPTIONS_HEADING = 'Subscriptions'
export const API_TOKENS_HEADING = 'API tokens'
export const SESSION_PIN_NOTE =
  'This chat runs on this token from its next start, and every time it is reopened or restored. Refresh restarts it now.'

/** Chat provider id → token tool (null = no tokens for that harness). */
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

export interface TokenOption {
  value: string
  label: string
  disabled?: boolean
  group?: string
  badge?: string
}

/** "Server default — Claude · Max · main" (or the workspace's token for a
 *  chat whose workspace has one). */
function defaultOptionLabel(tool: LlmTool, workspaceDefaultId: string | null): string {
  if (workspaceDefaultId) {
    const ws = tool.accounts.find((a) => a.id === workspaceDefaultId)
    return ws ? `${WORKSPACE_DEFAULT_LABEL} — ${tokenName(tool.display, ws)}` : WORKSPACE_DEFAULT_LABEL
  }
  const def = serverDefaultAccount(tool)
  return def ? `${SERVER_DEFAULT_LABEL} — ${tokenName(tool.display, def)}` : SERVER_DEFAULT_LABEL
}

/** The open menu: the default first, then Subscriptions, then API tokens.
 *  The entry in use carries the "In use" badge. */
export function tokenOptions(tool: LlmTool, value: string, workspaceDefaultId: string | null = null): TokenOption[] {
  const badge = (v: string): string | undefined => (v === value ? IN_USE : undefined)
  const opts: TokenOption[] = [
    { value: DEFAULT_VALUE, label: defaultOptionLabel(tool, workspaceDefaultId), badge: badge(DEFAULT_VALUE) },
  ]
  const subs = tool.accounts.filter((a) => a.kind !== 'api_key')
  const keys = tool.accounts.filter((a) => a.kind === 'api_key')
  for (const a of subs) {
    const block = pinBlockReason(tool, a)
    const name = tokenName(tool.display, a)
    opts.push({
      value: a.id,
      label: block ? `${name} — ${block}` : name,
      disabled: block !== null,
      group: SUBSCRIPTIONS_HEADING,
      badge: badge(a.id),
    })
  }
  for (const a of keys) {
    opts.push({ value: a.id, label: tokenName(tool.display, a), group: API_TOKENS_HEADING, badge: badge(a.id) })
  }
  return opts
}

/** The name of the token in use, for the accessible label. */
function inUseName(tool: LlmTool, value: string, workspaceDefaultId: string | null): string {
  if (value === DEFAULT_VALUE) return defaultOptionLabel(tool, workspaceDefaultId)
  const a = tool.accounts.find((x) => x.id === value)
  return a ? tokenName(tool.display, a) : value
}

function useDoc(scope: ServerScope): LlmAccountsDoc | null {
  const entry = useLlmAccountsStore((s) => s.entries[scope.id])
  useEffect(() => watchLlmAccounts(scope), [scope])
  return entry?.doc ?? null
}

/** Pick a token for a scope: the default unpins, anything else pins. */
function usePick(
  scope: ServerScope,
  doc: LlmAccountsDoc,
  tool: LlmTool,
  pinScope: PinScope,
  scopeId: string,
): { value: string; choose: (v: string) => Promise<void>; error: string | null; note: string | null } {
  const pin = pinFor(doc, pinScope, scopeId, tool.tool)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const choose = async (value: string): Promise<void> => {
    setError(null)
    setNote(null)
    try {
      if (value === DEFAULT_VALUE) {
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
  return { value: pin ? pin.accountId : DEFAULT_VALUE, choose, error, note }
}

function Notes({
  tool,
  value,
  note,
  error,
}: {
  tool: LlmTool
  value: string
  note: string | null
  error: string | null
}): React.JSX.Element {
  const picked = tool.accounts.find((a) => a.id === value)
  return (
    <>
      {picked?.kind === 'api_key' && (
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">This API token is billed per token.</p>
      )}
      {note && <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{note}</p>}
      {error && (
        <p className="text-[10px] text-red-400 mt-0.5" role="alert">
          {error}
        </p>
      )}
    </>
  )
}

function WorkspaceTokenRow({
  scope,
  doc,
  tool,
  projectId,
}: {
  scope: ServerScope
  doc: LlmAccountsDoc
  tool: LlmTool
  projectId: string
}): React.JSX.Element {
  const { value, choose, error, note } = usePick(scope, doc, tool, 'workspace', projectId)
  return (
    <div className="py-1.5" data-testid={`llm-pin-workspace-${tool.tool}`}>
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs text-[var(--color-text-secondary)]">{tool.display}</span>
        <SettingDropdown
          value={value}
          options={tokenOptions(tool, value)}
          onChange={(v) => choose(v)}
          triggerLabel={TOKEN_LABEL}
          ariaLabel={`${tool.display} token: ${inUseName(tool, value, null)}`}
        />
      </div>
      <Notes tool={tool} value={value} note={note} error={error} />
    </div>
  )
}

/** Workspace settings → Agent → "LLM tokens": one Token picker per tool. */
export function WorkspaceLlmTokens({ scope, projectId }: { scope: ServerScope; projectId: string }): React.JSX.Element {
  const doc = useDoc(scope)
  const tools = (doc?.tools ?? []).filter((t) => t.supported)
  return (
    <div data-settings-id="projects.llm-logins" data-testid="workspace-llm-tokens">
      <p className="text-[10px] text-[var(--color-text-muted)] mb-1 leading-relaxed">
        Server default uses this server&apos;s default token (Settings → LLMs). Pick a subscription or API token to
        use this token for this workspace&apos;s agents and sidecars instead. Resumed chats keep the token they
        started on.
      </p>
      {!doc && <p className="text-[10px] text-[var(--color-text-muted)]">Loading tokens…</p>}
      {doc && tools.map((t) => <WorkspaceTokenRow key={t.tool} scope={scope} doc={doc} tool={t} projectId={projectId} />)}
    </div>
  )
}

/** The open list in the chat header: headings, check + "In use" on the
 *  token in use. */
function TokenList({
  options,
  value,
  onPick,
}: {
  options: TokenOption[]
  value: string
  onPick: (v: string) => void
}): React.JSX.Element {
  return (
    <div data-testid="chat-token-list">
      {options.map((o, i) => {
        const heading = o.group !== undefined && o.group !== options[i - 1]?.group ? o.group : null
        const active = o.value === value
        return (
          <React.Fragment key={o.value}>
            {heading !== null && (
              <div className="pt-1.5 pb-0.5 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">{heading}</div>
            )}
            <button
              type="button"
              disabled={o.disabled === true}
              onClick={() => {
                if (!o.disabled && !active) onPick(o.value)
              }}
              className={`w-full flex items-center gap-2 px-1.5 py-1 text-left text-[11px] ${
                o.disabled
                  ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                  : active
                    ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10'
                    : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
              }`}
            >
              <span className="flex-1 min-w-0 break-words">{o.label}</span>
              {o.badge && (
                <span className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] flex-shrink-0">{o.badge}</span>
              )}
              {active && (
                <svg
                  className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                  strokeWidth={2.5}
                  aria-hidden="true"
                >
                  <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                </svg>
              )}
            </button>
          </React.Fragment>
        )
      })}
    </div>
  )
}

/** One chat: its v2 session key (the pinned chat: the workspace id; a tab:
 *  `tab-<terminalId>`), its conversation id when known, and its
 *  workspace (for the "Workspace default" entry). */
export interface ChatTokenTarget {
  sessionKey: string
  conversationId?: string | null
  projectId?: string | null
}

/** The chat's state for one tool: the value in use (a token id or
 *  DEFAULT_VALUE), the pick behind it, and the workspace default. */
export function chatTokenState(
  doc: LlmAccountsDoc,
  tool: LlmTool,
  target: ChatTokenTarget,
): { value: string; pin: LlmPin | null; workspaceDefaultId: string | null } {
  const pin = chatPinFor(doc, target.sessionKey, target.conversationId, tool.tool)
  const wsPin = target.projectId ? pinFor(doc, 'workspace', target.projectId, tool.tool) : null
  return { value: pin ? pin.accountId : DEFAULT_VALUE, pin, workspaceDefaultId: wsPin ? wsPin.accountId : null }
}

/** The tool a chat's harness uses when it has saved tokens on that
 *  server, else null (no Token control: shells, harnesses without
 *  tokens, a tool with nothing saved). */
export function chatTokenTool(doc: LlmAccountsDoc | null, provider: string | null | undefined): LlmTool | null {
  const toolId = toolForProvider(provider)
  const tool = doc?.tools.find((t) => t.tool === toolId && t.supported)
  return tool && tool.accounts.length > 0 ? tool : null
}

/** Pick a token for one chat: the default unpins its key and its
 *  conversation; anything else pins both. Resolves to the note to show
 *  (Claude subscription: its history is copied over), or null. */
export async function chooseChatToken(
  scope: ServerScope,
  tool: LlmTool,
  target: ChatTokenTarget,
  current: LlmPin | null,
  value: string,
): Promise<string | null> {
  const conv = target.conversationId?.trim() || null
  if (value === DEFAULT_VALUE) {
    if (current) await unpinLogin(scope, 'session', target.sessionKey, tool.tool, conv)
    return null
  }
  await pinLogin(scope, 'session', target.sessionKey, tool.tool, value, conv)
  const acc = tool.accounts.find((a) => a.id === value)
  return tool.tool === 'claude' && acc?.kind !== 'api_key' ? CLAUDE_CHAT_PIN_NOTE : null
}

/** Item ids of the tab menu's Token submenu: `token:<value>`. */
export const TOKEN_MENU_PREFIX = 'token:'

/** The tab right-click "Token ▸" submenu: the same entries as the header
 *  menu (default, Subscriptions, API tokens), the one in use checked and
 *  marked "In use", then the note. */
export function chatTokenMenuItems(
  tool: LlmTool,
  value: string,
  workspaceDefaultId: string | null,
): ContextMenuItemDef[] {
  const items: ContextMenuItemDef[] = []
  let group: string | undefined
  for (const o of tokenOptions(tool, value, workspaceDefaultId)) {
    if (o.group !== undefined && o.group !== group) {
      items.push({ id: `token-group:${o.group}`, label: o.group, type: 'heading' })
    }
    group = o.group
    items.push({
      id: `${TOKEN_MENU_PREFIX}${o.value}`,
      label: o.label,
      checked: o.value === value,
      badge: o.badge,
      enabled: o.disabled !== true,
    })
  }
  items.push({ id: 'token-note-sep', label: '', type: 'separator' })
  items.push({ id: 'token-note', label: SESSION_PIN_NOTE, type: 'note' })
  return items
}

function SessionTokenMenu({
  scope,
  doc,
  tool,
  target,
}: {
  scope: ServerScope
  doc: LlmAccountsDoc
  tool: LlmTool
  target: ChatTokenTarget
}): React.JSX.Element {
  const { value, pin, workspaceDefaultId } = chatTokenState(doc, tool, target)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const choose = async (v: string): Promise<void> => {
    setError(null)
    setNote(null)
    try {
      setNote(await chooseChatToken(scope, tool, target, pin, v))
    } catch (e) {
      setError(errorText(e))
    }
  }
  return (
    <div className="space-y-1">
      <p className="text-[10px] text-[var(--color-text-muted)]">Use this token for this chat</p>
      <TokenList options={tokenOptions(tool, value, workspaceDefaultId)} value={value} onPick={(v) => void choose(v)} />
      <Notes tool={tool} value={value} note={note} error={error} />
      {value !== DEFAULT_VALUE && (
        <p className="text-[10px] text-[var(--color-text-muted)] pt-1 border-t border-[var(--color-border)]">
          Hit a limit on this token? Pick another one above.
        </p>
      )}
      <p className="text-[10px] text-[var(--color-text-muted)]">{SESSION_PIN_NOTE}</p>
    </div>
  )
}

/** Chat header "Token" control. The pinned chat passes only `projectId`
 *  (its session key is the workspace id); an agent chat tab passes its
 *  `sessionKey` (`tab-<terminalId>`) and `conversationId`. Hidden when
 *  the tool has no saved tokens. */
export function SessionTokenPicker({
  scope,
  projectId,
  provider,
  sessionKey,
  conversationId,
  resolveProjectId,
}: {
  scope: ServerScope
  projectId?: string | null
  provider: string | null | undefined
  sessionKey?: string
  conversationId?: string | null
  /** A tab's workspace, looked up only once the control shows. */
  resolveProjectId?: () => string | null
}): React.JSX.Element | null {
  const doc = useDoc(scope)
  const [open, setOpen] = useState(false)
  const tool = chatTokenTool(doc, provider)
  const key = sessionKey || projectId || ''
  if (!doc || !tool || !key) return null
  const target: ChatTokenTarget = {
    sessionKey: key,
    conversationId,
    projectId: projectId ?? resolveProjectId?.() ?? null,
  }
  const { value, workspaceDefaultId } = chatTokenState(doc, tool, target)
  return (
    <div className="relative self-center flex items-center" data-testid="chat-login-picker">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-label={`${tool.display} token for this chat: ${inUseName(tool, value, workspaceDefaultId)}`}
        aria-expanded={open}
        className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer"
      >
        <span>{TOKEN_LABEL}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
          <path d={open ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
        </svg>
      </button>
      {open && (
        <div className="absolute right-0 top-full mt-1 z-30 w-[40ch] bg-[var(--color-bg)] border border-[var(--color-border)] shadow-2xl p-2">
          <SessionTokenMenu scope={scope} doc={doc} tool={tool} target={target} />
        </div>
      )}
    </div>
  )
}
