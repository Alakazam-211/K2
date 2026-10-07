import type { SessionViewTab } from './sessionViewTab'
import { chatHarnessName } from './chatHarness'

export type ChatBlock =
  | { type: 'text'; text: string }
  | { type: 'tool_call'; id: string; name: string; input: string }
  | { type: 'tool_result'; id: string; content: string }
  /** Thinking / reasoning (TW10). `text` only when the harness wrote it
   *  in the clear; `duration` is ms since the record before it. */
  | { type: 'thinking'; text?: string | null; redacted: boolean; duration?: number | null }

export interface ChatTurn {
  id: string
  role: string
  time?: string | null
  blocks: ChatBlock[]
}

export type ChatTranscriptFrame =
  | { kind: 'snapshot'; turns: ChatTurn[] }
  | { kind: 'reset' }
  | { kind: 'turn'; turn: ChatTurn }

/** Open only while Chat is the visible view and the provider id exists. */
export function shouldOpenChatTranscriptSocket(opts: {
  view: SessionViewTab
  visible: boolean
  provider: string | null
  conversationId: string | null
}): boolean {
  if (opts.view !== 'chat' || !opts.visible) return false
  if (!chatHarnessName({ provider: opts.provider })) return false
  return Boolean(opts.conversationId?.trim())
}

/**
 * Daemon transcript socket. Query is provider, conversation id, and the
 * v2 agent name. Not a filesystem path, and not grid / session-events / overlay.
 */
export function chatTranscriptWsUrl(
  base: string,
  token: string,
  query: { provider: string; conversationId: string; agentName?: string | null },
): string {
  const params = new URLSearchParams()
  params.set('provider', query.provider)
  params.set('conversation', query.conversationId)
  const agent = query.agentName?.trim()
  if (agent) params.set('agent', agent)
  params.set('token', token)
  return `${base}/cli/chat/transcript?${params.toString()}`
}

export function applyChatTranscriptFrame(prev: ChatTurn[], frame: ChatTranscriptFrame): ChatTurn[] {
  if (frame.kind === 'reset') return []
  if (frame.kind === 'snapshot') return frame.turns.slice()
  const turn = frame.turn
  if (!turn?.id) return prev
  const idx = prev.findIndex((item) => item.id === turn.id)
  if (idx < 0) return [...prev, turn]
  const next = prev.slice()
  next[idx] = turn
  return next
}

export function parseChatTranscriptFrame(raw: string): ChatTranscriptFrame | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== 'object') return null
  const kind = (parsed as { kind?: unknown }).kind
  if (kind === 'reset') return { kind: 'reset' }
  if (kind === 'snapshot') {
    const turns = (parsed as { turns?: unknown }).turns
    if (!Array.isArray(turns)) return null
    return { kind: 'snapshot', turns: turns.filter(isChatTurn) }
  }
  if (kind === 'turn') {
    const turn = (parsed as { turn?: unknown }).turn
    if (!isChatTurn(turn)) return null
    return { kind: 'turn', turn }
  }
  return null
}

function isChatTurn(value: unknown): value is ChatTurn {
  if (!value || typeof value !== 'object') return false
  const turn = value as { id?: unknown; role?: unknown; blocks?: unknown }
  return typeof turn.id === 'string' && typeof turn.role === 'string' && Array.isArray(turn.blocks)
}
