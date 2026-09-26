import { useLayoutEffect, useRef, type JSX } from 'react'
import type { ChatBlock, ChatTurn } from './chatTranscript'
import type { SessionViewTab } from './sessionViewTab'
import { useChatTranscript } from './useChatTranscript'

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
        turns.map((turn) => <ChatTurnRow key={turn.id} turn={turn} />)
      )}
    </div>
  )
}

function ChatTurnRow({ turn }: { turn: ChatTurn }): JSX.Element {
  const label = turnLabel(turn)
  const copyText = turn.blocks
    .map((block) => blockPlain(block))
    .filter((part) => part.length > 0)
    .join('\n')
  return (
    <article className="flex flex-col gap-1 min-w-0" data-testid="chat-turn" data-turn-id={turn.id}>
      <div className="flex items-center gap-2 text-[10px] text-[var(--color-text-muted)]">
        <span className="font-medium text-[var(--color-text-secondary)]">{label}</span>
        {turn.time ? <time dateTime={turn.time}>{turn.time}</time> : null}
        {copyText ? (
          <button
            type="button"
            className="ml-auto cursor-pointer hover:text-[var(--color-text-primary)]"
            onClick={() => {
              void navigator.clipboard?.writeText(copyText)
            }}
          >
            Copy
          </button>
        ) : null}
      </div>
      {turn.blocks.map((block, index) => (
        <ChatBlockView key={`${turn.id}:${index}`} block={block} />
      ))}
    </article>
  )
}

function turnLabel(turn: ChatTurn): string {
  if (turn.blocks.length > 0 && turn.blocks.every((block) => block.type === 'tool_result')) {
    return 'Tool'
  }
  if (turn.role === 'assistant') return 'Agent'
  if (turn.role === 'tool') return 'Tool'
  if (turn.role === 'user') return 'You'
  return turn.role
}

function blockPlain(block: ChatBlock): string {
  if (block.type === 'text') return block.text
  if (block.type === 'tool_call') return `${block.name}\n${block.input}`
  return block.content
}

function ChatBlockView({ block }: { block: ChatBlock }): JSX.Element {
  if (block.type === 'text') {
    return (
      <p className="text-[12px] text-[var(--color-text-primary)] whitespace-pre-wrap break-words m-0">
        {block.text}
      </p>
    )
  }
  if (block.type === 'tool_call') {
    return (
      <pre className="text-[11px] text-[var(--color-text-secondary)] whitespace-pre-wrap break-words m-0 font-mono">
        {block.name}
        {block.input ? `\n${block.input}` : ''}
      </pre>
    )
  }
  return (
    <pre className="text-[11px] text-[var(--color-text-secondary)] whitespace-pre-wrap break-words m-0 font-mono">
      {block.content}
    </pre>
  )
}
