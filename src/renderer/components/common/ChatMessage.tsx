// Shared thread body for Project chat + Feedback comments.
// Renders markdown (GFM) inside the selectable wrapper Tickets/Project
// chat already use — WKWebView otherwise swallows selection.

import {
  memo,
  useCallback,
  type CSSProperties,
  type JSX,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
} from 'react'
import remarkGfm from 'remark-gfm'
import Markdown from '@/components/Markdown/Markdown'
import {
  SELECTABLE_TEXT_STYLE,
  clearStuckBodyUserSelect,
} from './SelectableText'

const REMARK_GFM = [remarkGfm] as const

function onBodyPointerDown(e: PointerEvent | MouseEvent): void {
  if ('button' in e && e.button !== 0) return
  clearStuckBodyUserSelect()
  e.stopPropagation()
}

export const ChatMessageBody = memo(function ChatMessageBody({
  text,
  className = '',
  style,
}: {
  text: string
  className?: string
  style?: CSSProperties
}): JSX.Element {
  const setRef = useCallback((node: HTMLDivElement | null) => {
    if (!node) return
    node.style.setProperty('user-select', 'text', 'important')
    node.style.setProperty('-webkit-user-select', 'text', 'important')
    node.style.setProperty('-webkit-app-region', 'no-drag')
    node.style.cursor = 'text'
  }, [])

  return (
    <div
      ref={setRef}
      className={`selectable-copy chat-thread-selectable markdown-content chat-markdown ${className}`.trim()}
      style={{ ...SELECTABLE_TEXT_STYLE, ...style }}
      onPointerDown={onBodyPointerDown}
      onMouseDown={onBodyPointerDown}
    >
      <Markdown remarkPlugins={REMARK_GFM}>{text}</Markdown>
    </div>
  )
})

export const ChatMessage = memo(function ChatMessage({
  author,
  isOwner,
  timeLabel,
  body,
  fontSize,
  footer,
}: {
  author: string
  isOwner: boolean
  timeLabel: string
  body: string
  fontSize?: number
  footer?: ReactNode
}): JSX.Element {
  return (
    <div
      className={`flex flex-col gap-1 px-2.5 py-2 ${
        isOwner ? 'bg-white/[0.03]' : 'bg-[var(--color-bg-inset)]'
      }`}
    >
      <div className="flex items-baseline gap-2">
        <span
          className={`text-[10px] font-semibold ${
            isOwner ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-secondary)]'
          }`}
        >
          {author}
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)] tabular-nums">{timeLabel}</span>
      </div>
      <ChatMessageBody
        text={body}
        className="text-[var(--color-text-primary)]"
        style={fontSize ? { fontSize } : undefined}
      />
      {footer}
    </div>
  )
})
