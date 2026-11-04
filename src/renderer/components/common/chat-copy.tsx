// Copy buttons in chat markdown (Thread bubbles, the Zen Conversation
// widget, Project chat, Feedback comments: everything that renders through
// `ChatMessageBody`).
//   - A fenced code block gets a Copy button at its top right (on hover; always
//     on touch). It copies the block's raw text: no fence, and the one trailing
//     newline markdown adds to every block trimmed.
//   - A link's right-click menu has "Copy link". With Settings → General →
//     "Show a copy button beside links on hover" on (off by default; this
//     install only), a small copy icon also shows beside it on hover.
// The clipboard write goes through the Tauri clipboard plugin (no
// user-activation rule in WKWebView), with navigator.clipboard as the
// fallback, as the terminal's copy does.

import { useCallback, useEffect, useRef, useState, type JSX, type MouseEvent, type ReactNode } from 'react'
import { writeText as tauriClipboardWriteText } from '@tauri-apps/plugin-clipboard-manager'
import { useContextMenuStore } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'
import { useChatLinkCopyStore } from '@/stores/chat-link-copy'

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

function CopyGlyph({ done }: { done: boolean }): JSX.Element {
  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} aria-hidden>
      {done ? (
        <path strokeLinecap="round" strokeLinejoin="round" d="M5 12.5l4.5 4.5L19 7.5" />
      ) : (
        <>
          <rect x="8" y="8" width="12" height="12" rx="2" />
          <path strokeLinecap="round" strokeLinejoin="round" d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
        </>
      )}
    </svg>
  )
}

/** `a` for react-markdown: the link. Right-click offers "Copy link" (the
 *  app's own menu, since the webview's is off). It handles its own menu, so
 *  a link inside a Thread message opens this one, not the message's. With
 *  the "copy button beside links" setting on (Rosson 2026-11-04; off by
 *  default), a copy icon also shows beside the link on hover, as in 0.46.7. */
export function ChatLink({
  node: _node,
  children,
  ...props
}: { node?: unknown; children?: ReactNode } & React.AnchorHTMLAttributes<HTMLAnchorElement>): JSX.Element {
  const showCopyButton = useChatLinkCopyStore((s) => s.enabled)
  const [copied, copy] = useCopied()
  const href = typeof props.href === 'string' ? props.href : ''
  if (!href) return <a {...props}>{children}</a>
  const link = (
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
  if (!showCopyButton) return link
  return (
    <span className="chat-md-link">
      {link}
      <button
        type="button"
        className="chat-md-link-copy"
        data-chat-copy="link"
        data-copied={copied ? '' : undefined}
        aria-label={copied ? 'Link copied' : 'Copy link'}
        title={copied ? 'Copied' : 'Copy link'}
        onPointerDown={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          stop(e)
          copy(href)
        }}
      >
        <CopyGlyph done={copied} />
      </button>
    </span>
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
