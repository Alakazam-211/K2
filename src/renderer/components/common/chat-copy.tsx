// Copy buttons in chat markdown (Thread bubbles, the Zen Conversation
// widget, Project chat, Feedback comments: everything that renders through
// `ChatMessageBody`).
//   - A fenced code block gets a Copy button at its top right (on hover; always
//     on touch). It copies the block's raw text: no fence, and the one trailing
//     newline markdown adds to every block trimmed.
//   - A link's right-click menu has "Copy link" (no hover button).
// The clipboard write goes through the Tauri clipboard plugin (no
// user-activation rule in WKWebView), with navigator.clipboard as the
// fallback, as the terminal's copy does.

import { useCallback, useEffect, useRef, useState, type JSX, type MouseEvent, type ReactNode } from 'react'
import { writeText as tauriClipboardWriteText } from '@tauri-apps/plugin-clipboard-manager'
import { useContextMenuStore } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'

/** The text a code block's Copy puts on the clipboard: the code as written,
 *  minus the single trailing newline markdown ends every block with. */
export function codeBlockCopyText(raw: string): string {
  return raw.endsWith('\n') ? raw.slice(0, -1) : raw
}

interface HastLike {
  type?: string
  value?: unknown
  children?: unknown
}

/** The plain text under a hast node (the `<pre>` react-markdown hands us). */
export function hastText(node: unknown): string {
  if (!node || typeof node !== 'object') return ''
  const n = node as HastLike
  if (n.type === 'text' && typeof n.value === 'string') return n.value
  if (!Array.isArray(n.children)) return ''
  let out = ''
  for (const child of n.children) out += hastText(child)
  return out
}

export async function copyToClipboard(text: string): Promise<void> {
  try {
    await tauriClipboardWriteText(text)
  } catch {
    await navigator.clipboard.writeText(text)
  }
}

const COPIED_MS = 1500

/** Copy `text`, then report "copied" for a moment. */
function useCopied(): [boolean, (text: string) => void] {
  const [copied, setCopied] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
    },
    [],
  )
  const copy = useCallback((text: string) => {
    copyToClipboard(text).then(
      () => {
        setCopied(true)
        if (timer.current) clearTimeout(timer.current)
        timer.current = setTimeout(() => setCopied(false), COPIED_MS)
      },
      (err: unknown) => console.warn('[chat] copy failed', err),
    )
  }, [])
  return [copied, copy]
}

function stop(e: MouseEvent): void {
  e.preventDefault()
  e.stopPropagation()
}

/** `pre` for react-markdown: the block plus its Copy button. */
export function ChatCodeBlock({
  node,
  children,
  ...props
}: { node?: unknown; children?: ReactNode } & React.HTMLAttributes<HTMLPreElement>): JSX.Element {
  const [copied, copy] = useCopied()
  return (
    <div className="chat-md-code" data-chat-code="">
      <pre {...props}>{children}</pre>
      <button
        type="button"
        className="chat-md-copy"
        data-chat-copy="code"
        data-copied={copied ? '' : undefined}
        aria-label={copied ? 'Copied' : 'Copy code'}
        title="Copy code"
        onPointerDown={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          stop(e)
          copy(codeBlockCopyText(hastText(node)))
        }}
      >
        {copied ? 'Copied' : 'Copy'}
      </button>
    </div>
  )
}

/** `a` for react-markdown: the link. Right-click offers "Copy link" (Rosson
 *  2026-10-31: no button on hover; the app's own menu, since the webview's
 *  is off). It handles its own menu, so a link inside a Thread message opens
 *  this one, not the message's. */
export function ChatLink({
  node: _node,
  children,
  ...props
}: { node?: unknown; children?: ReactNode } & React.AnchorHTMLAttributes<HTMLAnchorElement>): JSX.Element {
  const href = typeof props.href === 'string' ? props.href : ''
  if (!href) return <a {...props}>{children}</a>
  return (
    <a
      {...props}
      data-chat-link=""
      onContextMenu={(e) => {
        props.onContextMenu?.(e)
        if (e.defaultPrevented) return
        e.preventDefault()
        e.stopPropagation()
        void openChatLinkMenu(e.clientX, e.clientY, href)
      }}
    >
      {children}
    </a>
  )
}

/** A link's right-click menu at (x, y): "Copy link". */
export async function openChatLinkMenu(x: number, y: number, href: string): Promise<void> {
  const picked = await useContextMenuStore.getState().show(x, y, [{ id: 'copy-link', label: 'Copy link' }])
  if (picked !== 'copy-link') return
  try {
    await copyToClipboard(href)
    useToastStore.getState().addToast('Link copied', 'success', 2000)
  } catch (err) {
    useToastStore.getState().addToast(`Couldn't copy the link: ${err instanceof Error ? err.message : String(err)}`, 'error')
  }
}
