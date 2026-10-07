import { useLayoutEffect, useRef, type JSX } from 'react'
import { ChatMessage } from '@/components/common/ChatMessage'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { chatHarnessLabel } from './chatHarness'
import type { ChatBlock, ChatTurn } from './chatTranscript'
import type { SessionViewTab } from './sessionViewTab'
import { useChatTranscript } from './useChatTranscript'
import { useRoom } from '@/components/Room/RoomContext'

export function ChatOverlayPane({
  view,
  visible,
  provider,
  conversationId,
  agentName,
}: {
  view: SessionViewTab
  visible: boolean
  provider: string | null
  conversationId: string | null
  agentName: string | null
}): JSX.Element {
  const { turns, error } = useChatTranscript({
    // Home M4: the room's server (B's thread in B's room).
    scope: useRoom().scope,
    view,
    visible,
    provider,
    conversationId,
    agentName,
  })
  const listRef = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    el.scrollTop = el.scrollHeight
  }, [turns])

  const waiting = !conversationId?.trim()
  return (
    <div
      ref={listRef}
      className="h-full min-h-0 overflow-y-auto px-3 py-2 flex flex-col gap-3"
      data-testid="chat-overlay-pane"
    >
      {error ? (
        <div className="text-[11px] text-[var(--color-text-muted)]">{error}</div>
      ) : null}
      {waiting ? (
        <div className="text-[11px] text-[var(--color-text-muted)]" data-testid="chat-overlay-waiting">
          Waiting for the session id…
        </div>
      ) : turns.length === 0 ? (
        <div className="text-[11px] text-[var(--color-text-muted)]">No messages yet.</div>
      ) : (
        <ChatTurnList turns={turns} harnessLabel={chatHarnessLabel(provider)} />
      )}
    </div>
  )
}

/** User and assistant text share Thread's ChatMessage. A tool call is its name, not its input.
 *  Thinking is a collapsed "Thought" stub under the message (TW10). */
export function ChatTurnList({
  turns,
  harnessLabel,
}: {
  turns: ChatTurn[]
  harnessLabel: string
}): JSX.Element {
  const nowSec = Math.floor(Date.now() / 1000)
  return (
    <>
      {turns.map((turn) => {
        const owner = isOwnerTurn(turn)
        return (
          <div key={turn.id} data-testid="chat-turn" data-turn-id={turn.id} data-role={turn.role}>
            <ChatMessage
              author={owner ? 'You' : harnessLabel}
              isOwner={owner}
              timeLabel={chatTimeLabel(turn.time, nowSec)}
              body={chatTurnBody(turn)}
              footer={<ThoughtStubs blocks={turn.blocks} />}
            />
          </div>
        )
      })}
    </>
  )
}

function isOwnerTurn(turn: ChatTurn): boolean {
  if (turn.role !== 'user') return false
  if (turn.blocks.length > 0 && turn.blocks.every((block) => block.type === 'tool_result')) return false
  return true
}

export function chatTurnBody(turn: ChatTurn): string {
  const lines: string[] = []
  for (const block of turn.blocks) {
    const line = blockLine(block)
    if (line) lines.push(line)
  }
  return lines.join('\n')
}

function blockLine(block: ChatBlock): string {
  if (block.type === 'text') return block.text
  if (block.type === 'tool_call') return block.name
  if (block.type === 'thinking') return ''
  return block.content
}

/** "Thought for 8s" (or "Thought"), with the thinking text folded inside
 *  when the harness wrote it in the clear. Redacted thinking is the label only. */
export function thoughtLabel(block: Extract<ChatBlock, { type: 'thinking' }>): string {
  const ms = typeof block.duration === 'number' ? block.duration : null
  if (ms === null || ms < 1000) return 'Thought'
  const secs = Math.round(ms / 1000)
  if (secs < 60) return `Thought for ${secs}s`
  return `Thought for ${Math.floor(secs / 60)}m ${secs % 60}s`
}

function ThoughtStubs({ blocks }: { blocks: ChatBlock[] }): JSX.Element | null {
  const thoughts = blocks.filter(
    (block): block is Extract<ChatBlock, { type: 'thinking' }> => block.type === 'thinking',
  )
  if (thoughts.length === 0) return null
  return (
    <div className="flex flex-col gap-0.5">
      {thoughts.map((block, i) => {
        const label = thoughtLabel(block)
        const text = block.text?.trim()
        return text ? (
          <details key={i} data-testid="chat-thought" className="text-[10px] text-[var(--color-text-muted)]">
            <summary className="cursor-pointer select-none">{label}</summary>
            <div className="whitespace-pre-wrap pl-3 pt-0.5">{text}</div>
          </details>
        ) : (
          <div key={i} data-testid="chat-thought" className="text-[10px] text-[var(--color-text-muted)]">
            {label}
          </div>
        )
      })}
    </div>
  )
}

function chatTimeLabel(time: string | null | undefined, nowSec: number): string {
  if (!time) return '—'
  const ms = Date.parse(time)
  if (Number.isNaN(ms)) return time
  return formatRelativeTime(Math.floor(ms / 1000), nowSec)
}
