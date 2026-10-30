// prd-thread-message-menu-v1 (Rosson 2026-10-30) — a Thread message's menu.
//
// Right-click (desktop), a long press (touch), or the ⋯ beside
// "author · time" opens it:
//   - Reply: the message waits as a chip above this Thread's message box
//     (a small icon and its first line, lighter; × or Esc cancels). The send
//     quotes it (lib/thread-reply: the format the agent reads).
//   - Reply to selection: when text is selected inside this one message;
//     the reply quotes exactly the selection.
//   - Copy text / Copy selection.
//   - Save to stash (spec-prompt-stash-v1).
// Reply shows only where this window has a message box for the Thread
// (the desktop Thread, a Zen conversation with its box).

import { useCallback, useEffect, useMemo, useRef, type JSX, type RefObject } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { copyToClipboard } from '@/components/common/chat-copy'
import { StashGlyph } from '@/components/PromptStash/PromptStash'
import { stashThreadText } from '@/components/PromptStash/StashFromThread'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'
import {
  threadReplyComposerOpen,
  threadReplyFirstLine,
  threadReplyQuote,
  useThreadReplyStore,
  type ParsedThreadReply,
  type ThreadReplyTarget,
} from '@/lib/thread-reply'

/** A press held this long (ms) without moving opens the menu (touch). */
export const THREAD_LONG_PRESS_MS = 500
/** A press that moves farther than this (px) is a scroll, not a long press. */
const LONG_PRESS_SLOP_PX = 10

/** What one message's menu acts on. */
export interface ThreadMessageMenuArgs {
  scope: ServerScope | null
  /** The message's words as drawn (Copy text, Save to stash). */
  text: string
  /** Stash source: the sender's address (or, on your own message, the Thread's). */
  agent: string | null
  agentName: string | null
  /** A reply message's quote, kept as a stash quote. */
  quote?: { text: string; from: string } | null
  /** The reply this message would start (null: no Reply). */
  reply: { key: string; target: ThreadReplyTarget } | null
}

/** The text selected inside `el` alone, else null (nothing selected, or a
 *  selection that runs outside this one message). */
export function selectionWithin(el: HTMLElement | null): string | null {
  const sel = typeof window !== 'undefined' ? window.getSelection?.() : null
  if (!el || !sel || sel.rangeCount === 0 || sel.isCollapsed) return null
  const text = sel.toString()
  if (!text.trim()) return null
  for (let i = 0; i < sel.rangeCount; i++) {
    if (!el.contains(sel.getRangeAt(i).commonAncestorContainer)) return null
  }
  return text
}

/** Is anything selected on the page? */
function anySelection(): boolean {
  const sel = typeof window !== 'undefined' ? window.getSelection?.() : null
  return !!sel && !sel.isCollapsed && sel.toString().length > 0
}

/** The menu's rows. `selection`: the text selected inside this message. */
export function threadMessageMenuItems(args: ThreadMessageMenuArgs, selection: string | null): ContextMenuItemDef[] {
  const reply = args.reply && threadReplyComposerOpen(args.reply.key)
  const items: ContextMenuItemDef[] = []
  if (reply && selection) items.push({ id: 'reply-selection', label: 'Reply to selection', icon: <ReplyGlyph size={12} /> })
  if (reply) items.push({ id: 'reply', label: 'Reply', icon: <ReplyGlyph size={12} /> })
  items.push({ id: 'copy', label: selection ? 'Copy selection' : 'Copy text', enabled: (selection ?? args.text).length > 0 })
  items.push({ id: 'sep', label: '', type: 'separator' })
  items.push({
    id: 'stash',
    label: 'Save to stash',
    icon: <StashGlyph size={12} />,
    enabled: args.text.trim().length > 0 || !!args.quote,
  })
  return items
}

/** A message's menu at (x, y), and what its pick does. */
export async function openThreadMessageMenu(
  x: number,
  y: number,
  args: ThreadMessageMenuArgs,
  selection: string | null = null,
): Promise<void> {
  const picked = await useContextMenuStore.getState().show(x, y, threadMessageMenuItems(args, selection))
  if (picked === 'reply' && args.reply) {
    useThreadReplyStore.getState().set(args.reply.key, args.reply.target)
  } else if (picked === 'reply-selection' && args.reply && selection) {
    useThreadReplyStore.getState().set(args.reply.key, { ...args.reply.target, text: selection, part: true })
  } else if (picked === 'copy') {
    try {
      await copyToClipboard(selection ?? args.text)
    } catch (err) {
      useToastStore.getState().addToast(`Couldn't copy: ${err instanceof Error ? err.message : String(err)}`, 'error')
    }
  } else if (picked === 'stash') {
    await stashThreadText({
      scope: args.scope,
      text: args.text,
      agent: args.agent,
      agentName: args.agentName,
      ...(args.quote ? { quotes: [args.quote] } : {}),
    })
  }
}

/**
 * The bubble's gestures for its menu: right-click (a selection inside this
 * message gets "Reply to selection"; one that runs outside it keeps the
 * webview's own menu), and a long press on touch. The selection is read when
 * the press starts, before a right-click or a long press can select a word.
 * `open` null: no menu (the handlers do nothing).
 */
export function useThreadMessageGestures(
  ref: RefObject<HTMLElement | null>,
  open: ((x: number, y: number, selection: string | null) => void) | null,
): {
  onPointerDown: (e: React.PointerEvent) => void
  onPointerMove: (e: React.PointerEvent) => void
  onPointerUp: () => void
  onPointerCancel: () => void
  onContextMenu: (e: React.MouseEvent) => void
} {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const start = useRef<{ x: number; y: number } | null>(null)
  /** The selection when the last press began: [time, text inside, any at all]. */
  const pressSel = useRef<{ at: number; inside: string | null; any: boolean } | null>(null)
  /** When a long press last opened the menu (a touch browser's own
   *  contextmenu right after is swallowed). */
  const firedAt = useRef(0)
  const cancel = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
    start.current = null
  }, [])
  useEffect(() => cancel, [cancel])
  const recentSelection = (): { inside: string | null; any: boolean } => {
    const p = pressSel.current
    if (p && Date.now() - p.at < 2000) return p
    return { inside: selectionWithin(ref.current), any: anySelection() }
  }
  return {
    onPointerDown: (e) => {
      if (!open) return
      pressSel.current = { at: Date.now(), inside: selectionWithin(ref.current), any: anySelection() }
      if (e.pointerType !== 'touch') return
      cancel()
      const x = e.clientX
      const y = e.clientY
      start.current = { x, y }
      timer.current = setTimeout(() => {
        timer.current = null
        start.current = null
        firedAt.current = Date.now()
        const sel = pressSel.current
        open(x, y, sel?.inside ?? null)
      }, THREAD_LONG_PRESS_MS)
    },
    onPointerMove: (e) => {
      const s = start.current
      if (!s) return
      if (Math.hypot(e.clientX - s.x, e.clientY - s.y) > LONG_PRESS_SLOP_PX) cancel()
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    onContextMenu: (e) => {
      if (!open) return
      if (Date.now() - firedAt.current < 1000) {
        e.preventDefault()
        return
      }
      const sel = recentSelection()
      pressSel.current = null
      // A selection across messages keeps the webview's own Copy menu.
      if (sel.any && !sel.inside) return
      e.preventDefault()
      open(e.clientX, e.clientY, sel.inside)
    },
  }
}

/** The ⋯ beside a message's "author · time": opens its menu. An SVG (no
 *  text), so the meta line reads the same everywhere. */
export function ThreadMessageMoreButton({ onOpen }: { onOpen: (x: number, y: number) => void }): JSX.Element {
  return (
    <button
      type="button"
      data-zen-message-more=""
      aria-label="Message menu"
      title="More"
      onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect()
        onOpen(r.left, r.bottom)
      }}
      className="cursor-pointer"
      style={{ background: 'none', border: 'none', padding: '0 4px', marginLeft: 4, color: 'inherit', verticalAlign: 'middle' }}
    >
      <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden style={{ display: 'inline-block' }}>
        <circle cx="5" cy="12" r="2" />
        <circle cx="12" cy="12" r="2" />
        <circle cx="19" cy="12" r="2" />
      </svg>
    </button>
  )
}

/** The reply arrow. */
export function ReplyGlyph({ size = 12 }: { size?: number }): JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" style={{ flexShrink: 0 }}>
      <polyline points="9 14 4 9 9 4" />
      <path d="M20 20v-7a4 4 0 0 0-4-4H4" />
    </svg>
  )
}

const MUTED = 'var(--zen-text-muted, var(--color-text-muted))'

/** A message box's reply, if any, for `key` (null: this box takes none).
 *  The box counts as open for `key` while mounted, so Reply shows. */
export function useThreadReply(key: string | null): { target: (ThreadReplyTarget & { nonce: number }) | null; clear: () => void } {
  useEffect(() => (key ? useThreadReplyStore.getState().register(key) : undefined), [key])
  const target = useThreadReplyStore((s) => (key ? (s.replies[key] ?? null) : null))
  const clear = useCallback(() => {
    if (key) useThreadReplyStore.getState().clear(key)
  }, [key])
  return useMemo(() => ({ target, clear }), [target, clear])
}

/** Above the message box: what the next send replies to, with its ×. */
export function ThreadReplyChip({ target, onCancel }: { target: ThreadReplyTarget; onCancel: () => void }): JSX.Element {
  const line = threadReplyFirstLine(threadReplyQuote(target))
  const what = target.part ? 'Replying to part of' : 'Replying to'
  return (
    <div
      data-testid="thread-reply-chip"
      data-zen-reply-chip=""
      data-reply-part={target.part ? '' : undefined}
      role="note"
      aria-label={`${what} ${target.from}: ${line}`}
      title={`${what} ${target.from}. Esc cancels.`}
      className="flex min-w-0 items-center gap-1.5"
      style={{
        marginBottom: 6,
        padding: '3px 4px 3px 10px',
        borderLeft: '2px solid var(--zen-accent, var(--color-accent))',
        fontSize: '0.8em',
        color: MUTED,
      }}
    >
      <ReplyGlyph size={12} />
      <span className="min-w-0 flex-1 truncate" data-reply-chip-text="" style={{ opacity: 0.85 }}>
        {line}
      </span>
      <button
        type="button"
        aria-label="Cancel the reply"
        data-testid="thread-reply-cancel"
        data-zen-soft-button=""
        onClick={(e) => {
          e.stopPropagation()
          onCancel()
        }}
        className="flex flex-shrink-0 cursor-pointer items-center justify-center"
        style={{ width: 18, height: 18, borderRadius: 999, background: 'none', border: 'none', color: MUTED }}
      >
        ×
      </button>
    </div>
  )
}

/** On a sent reply, inside its bubble: the quoted message's first line. */
export function ThreadReplyQuote({ reply }: { reply: ParsedThreadReply }): JSX.Element {
  const line = threadReplyFirstLine(reply.quote)
  return (
    <div
      data-thread-reply-quote=""
      title={reply.quote}
      className="flex min-w-0 items-center gap-1.5"
      style={{
        marginBottom: 4,
        paddingLeft: 8,
        borderLeft: '2px solid currentColor',
        fontSize: '0.82em',
        opacity: 0.7,
      }}
    >
      <ReplyGlyph size={11} />
      <span className="min-w-0 truncate">
        {reply.part ? `${reply.from} (part): ` : `${reply.from}: `}
        {line}
      </span>
    </div>
  )
}
