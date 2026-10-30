// Shared thread body for Project chat + Feedback comments.
// Renders markdown (GFM) inside the selectable wrapper Tickets/Project
// chat already use — WKWebView otherwise swallows selection.

import {
  memo,
  useCallback,
  useMemo,
  type CSSProperties,
  type JSX,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
} from 'react'
import remarkGfm from 'remark-gfm'
import type { Components, Options } from 'react-markdown'
import Markdown from '@/components/Markdown/Markdown'
import {
  SELECTABLE_TEXT_STYLE,
  clearStuckBodyUserSelect,
} from './SelectableText'
import { MarkdownSpan, rehypeFileLinks, useFileLinkPrefixes } from './file-links'
import { ChatCodeBlock, ChatLink } from './chat-copy'

const CHAT_REMARK = [remarkGfm]

/** GFM tables scroll sideways in their own box instead of crushing a
 *  short column to one letter per line (`.chat-md-table-wrap` in
 *  globals.css). Code blocks get a Copy button and links a copy icon
 *  (./chat-copy). */
const CHAT_COMPONENTS: Components = {
  table: ({ node: _node, ...props }) => (
    <div className="chat-md-table-wrap">
      <table {...props} />
    </div>
  ),
  pre: ChatCodeBlock as Components['pre'],
  a: ChatLink as Components['a'],
}

/** Under a `FileLinkProvider` (the Thread): workspace paths become links. */
const CHAT_COMPONENTS_WITH_FILE_LINKS: Components = {
  ...CHAT_COMPONENTS,
  span: MarkdownSpan as Components['span'],
}

/** GFM treats a single \n as a space. Chat Shift+Enter is a hard break
 *  (`  \\n`). No extra remark plugin — Vite on tauri-dev failed to
 *  resolve `remark-breaks` even after bun install. Lines inside a fenced
 *  code block are left alone: there the two spaces would be part of the
 *  code (and of what its Copy button copies). */
export function chatHardBreaks(text: string): string {
  const lines = text.split('\n')
  let fence: { char: string; len: number } | null = null
  let out = ''
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    // CommonMark fences: up to 3 spaces, then 3+ backticks or tildes; a
    // backtick fence's info string has no backtick; the closing fence is the
    // same character, at least as long, and nothing after it but spaces.
    const m = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line)
    let inCode = fence !== null
    if (fence) {
      if (m && m[1][0] === fence.char && m[1].length >= fence.len && m[2].trim() === '') fence = null
    } else if (m && !(m[1][0] === '`' && m[2].includes('`'))) {
      fence = { char: m[1][0], len: m[1].length }
      inCode = true
    }
    if (i < lines.length - 1) out += line + (inCode ? '\n' : '  \n')
    else out += line
  }
  return out
}

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
  const fileLinks = useFileLinkPrefixes()
  const rehypePlugins = useMemo<Options['rehypePlugins']>(
    () => (fileLinks.enabled ? [[rehypeFileLinks, { prefixes: fileLinks.prefixes }]] : undefined),
    [fileLinks.enabled, fileLinks.prefixes],
  )

  return (
    <div
      ref={setRef}
      className={`selectable-copy chat-thread-selectable markdown-content chat-markdown ${className}`.trim()}
      style={{ ...SELECTABLE_TEXT_STYLE, ...style }}
      onPointerDown={onBodyPointerDown}
      onMouseDown={onBodyPointerDown}
    >
      <Markdown
        remarkPlugins={CHAT_REMARK}
        rehypePlugins={rehypePlugins}
        components={fileLinks.enabled ? CHAT_COMPONENTS_WITH_FILE_LINKS : CHAT_COMPONENTS}
      >
        {chatHardBreaks(text)}
      </Markdown>
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
      className={`flex flex-col gap-1 min-w-0 w-full px-2.5 py-2 ${
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
