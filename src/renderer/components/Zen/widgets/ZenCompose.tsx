// prd-zen-mode-v1 Z42, Z32 (answer 6) — the message box under a Zen
// conversation.
//
// Enter sends, Shift+Enter adds a line, Esc clears. "+" picks files on this
// computer with the same native picker as the Agents page's "message the
// agent" box; files dropped on the box are attached too. Sending goes through
// the bridge (`thread.post`, cap `thread:post`), which runs the existing
// attach path (local paths as they are, or an upload to the agent's own
// server) and the one compose send. A failed send puts the text back.
//
// Drafts survive switching conversations (this window only). A draft set
// from outside (`compose.draft`, prd-zen-gardens-v1 G28/G60: Ask my agent)
// shows at once, with the caret at the end of its first line, and is never
// sent until the person sends it.
//
// When the person picks this agent (a row click, or Add agent), the box
// takes keyboard focus once it can be typed in (`zen-compose-focus`).
// Nothing else focuses it: not first render, not a remote update.
//
// 0.46.0 (Rosson 2026-10-16): past 3 lines the box stacks like the desktop
// Thread's (the shared rule in ThreadZenKit), on by default.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { pickLocalComposeFiles } from '@/lib/pick-compose-files'
import {
  __resetZenComposeDraftsForTests,
  keepZenComposeDraft,
  onZenComposeDraft,
  zenComposeDraft,
} from '@/lib/zen/zen-compose-drafts'
import { takeZenComposeFocus, useZenComposeFocusStore } from '@/lib/zen/zen-compose-focus'
import { useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import {
  clearZenThreadContextNote,
  removeZenThreadContext,
  useZenConversationContext,
} from '@/lib/zen/zen-thread-context'
import { ThreadContextComposeChip, ThreadContextNote } from '@/components/Thread/ThreadContextChip'
import { ThreadReplyChip, useThreadReply } from '@/components/Thread/ThreadMessageMenu'
import { formatThreadReply } from '@/lib/thread-reply'
import {
  ThreadAttachmentChip,
  ThreadAttachmentChips,
  ThreadComposeBox,
  ThreadComposeStackSpacer,
  ThreadRoundButton,
  ThreadSendButton,
  threadComposeLines,
  threadComposeStackNext,
  threadComposeTextSlotStyle,
} from '@/components/Thread/ThreadZenKit'

/** The box's textarea padding, top + bottom (px). */
const ZEN_COMPOSE_PAD_Y = 12
/** The box's gap between its children (`gap-2`), and its buttons' side. */
const ZEN_COMPOSE_GAP = 8
const ZEN_COMPOSE_BUTTON = 32

/** Width (px) the one-row layout takes beside the textarea: "+" (when
 *  attachments are on), the box's gaps, and Send. */
export function zenComposeRowChrome(attachments: boolean): number {
  return (attachments ? ZEN_COMPOSE_BUTTON + ZEN_COMPOSE_GAP : 0) + ZEN_COMPOSE_GAP + ZEN_COMPOSE_BUTTON
}

/** One line of the box's text (px): its computed line height, else its font
 *  size × 1.4 (a `normal` line height), else 14 × 1.4. */
function zenComposeLinePx(el: HTMLElement): number {
  const cs = getComputedStyle(el)
  if (cs.lineHeight.endsWith('px')) {
    const lh = parseFloat(cs.lineHeight)
    if (Number.isFinite(lh) && lh > 0) return lh
  }
  const fs = parseFloat(cs.fontSize)
  return (Number.isFinite(fs) && fs > 0 ? fs : 14) * 1.4
}

type Attachment = { kind: 'path'; path: string; name: string } | { kind: 'file'; file: File; name: string }

const baseName = (p: string): string => p.split(/[/\\]/).pop() || p

export function ZenCompose({
  bridge,
  address,
  label,
  disabled,
  attachments: allowAttachments = true,
  onSend,
  drawer,
  stack = true,
  replyKey = null,
}: {
  bridge: ZenWidgetBridge
  address: string
  label: string
  disabled: boolean
  /** The `attachments` prop: "+" and drops (default on). */
  attachments?: boolean
  /** A send started (the conversation scrolls to its bottom). */
  onSend?: () => void
  /** Drawn on the box's top edge (Garden 1's activity drawer). */
  drawer?: React.ReactNode
  /** The desktop Thread's stacked layout past 3 lines (default on). */
  stack?: boolean
  /** prd-thread-message-menu-v1: the key a message's Reply waits under for
   *  this conversation (`threadReplyKey` of its server and Thread address);
   *  null: this box takes no replies. */
  replyKey?: string | null
}): React.JSX.Element {
  const [draft, setDraftState] = useState(() => zenComposeDraft(address))
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const ref = useRef<HTMLTextAreaElement | null>(null)
  // thread-context-v1 (§4): the Board context a followed widget set, shown
  // as a removable chip; exactly what the chip reads rides the next send.
  const gardenId = bridge.gardens?.current?.()?.id ?? ''
  const boardContext = useZenConversationContext(gardenId, bridge.widgetId)
  // A message's Reply: a chip above the box; it rides the next send as a quote.
  const reply = useThreadReply(replyKey)
  const replyNonce = reply.target?.nonce
  useEffect(() => {
    if (replyNonce === undefined || disabled) return
    ref.current?.focus()
  }, [replyNonce, disabled])

  const setDraft = useCallback(
    (next: string | ((cur: string) => string)) => {
      setDraftState((cur) => {
        const v = typeof next === 'function' ? next(cur) : next
        keepZenComposeDraft(address, v)
        return v
      })
    },
    [address],
  )

  // A draft from outside (Ask my agent): show it, caret after the first line.
  const caretAt = useRef<number | null>(null)
  useEffect(
    () =>
      onZenComposeDraft(address, (text) => {
        const nl = text.indexOf('\n')
        caretAt.current = nl < 0 ? text.length : nl
        setDraftState(text)
      }),
    [address],
  )
  useEffect(() => {
    const el = ref.current
    const at = caretAt.current
    if (!el || at === null || el.disabled) return
    caretAt.current = null
    el.focus()
    el.setSelectionRange(at, at)
  })

  // A pick of this agent: focus the box once it can be typed in.
  const focusRequest = useZenComposeFocusStore((s) => s.request)
  useEffect(() => {
    const el = ref.current
    if (!el || disabled || focusRequest?.address !== address) return
    if (!takeZenComposeFocus(address, el)) return
    el.focus({ preventScroll: true })
    const end = el.value.length
    el.setSelectionRange(end, end)
  }, [focusRequest, address, disabled])

  // Auto-grow up to ~8 lines, before paint (so the list above re-pins in
  // the same frame). Measuring needs `height: auto` for a moment; the row
  // around the box keeps its height meanwhile, or the box would collapse to
  // one line in that forced layout, the list above would grow, and the
  // browser would clamp its scrollTop: the newest message and the working
  // dots would slip under the box on every keystroke (Rosson 2026-10-04).
  //
  // Past 3 lines the box stacks like the desktop Thread's (ThreadZenKit:
  // the text on its own full-width line, "+" and Send under it); back to one
  // row once the text would fit in 2 lines of the one-row field. Re-measured
  // when the layout flips (the field's width changed); the rule's
  // hysteresis settles it in one step. A hidden box (no width) keeps its
  // layout.
  const [stacked, setStacked] = useState(false)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const row = el.parentElement
    const held = row ? row.style.minHeight : ''
    if (row) row.style.minHeight = `${row.offsetHeight}px`
    el.style.height = 'auto'
    const scrollHeight = el.scrollHeight
    el.style.height = `${Math.min(scrollHeight, 180)}px`
    if (row) row.style.minHeight = held
    if (!stack) {
      setStacked(false)
      return
    }
    const width = el.clientWidth
    if (width <= 0) return
    const lines = threadComposeLines({ value: el.value, scrollHeight }, zenComposeLinePx(el), ZEN_COMPOSE_PAD_Y)
    setStacked((cur) => threadComposeStackNext({ stacked: cur, lines, width, rowChrome: zenComposeRowChrome(allowAttachments) }))
  }, [draft, stacked, stack, allowAttachments])

  const send = useCallback(async () => {
    if (disabled || sending) return
    const text = draft.trim()
    if (!text && attachments.length === 0) return
    const paths = attachments.flatMap((a) => (a.kind === 'path' ? [a.path] : []))
    const files = attachments.flatMap((a) => (a.kind === 'file' ? [a.file] : []))
    const held = attachments
    const contextText = boardContext.context?.text
    const replyTo = reply.target
    // "Recent agents first" (Gardens): this computer spoke to this agent
    // now. Recorded whether the setting is on or not.
    useZenRecentFirstStore.getState().markSent(address)
    onSend?.()
    setSending(true)
    setError(null)
    setDraft('')
    setAttachments([])
    try {
      // A Reply: the quote and its reference line, then the words.
      await (bridge.call('thread.post', address, replyTo ? formatThreadReply(replyTo, text) : text, {
        ...(paths.length > 0 ? { paths } : {}),
        ...(files.length > 0 ? { files } : {}),
        ...(contextText ? { context: contextText } : {}),
      }) as Promise<unknown>)
      // The chip goes with the send (a refused send keeps it).
      if (replyTo) reply.clear()
    } catch (err) {
      setDraft((cur) => (cur.length === 0 ? text : cur))
      setAttachments((cur) => (cur.length === 0 ? held : cur))
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSending(false)
    }
  }, [address, attachments, boardContext.context, bridge, disabled, draft, onSend, reply, sending, setDraft])

  const pick = useCallback(() => {
    void pickLocalComposeFiles()
      .then((paths) => {
        if (!paths || paths.length === 0) return
        setAttachments((cur) => [...cur, ...paths.map((path) => ({ kind: 'path' as const, path, name: baseName(path) }))])
        ref.current?.focus()
      })
      .catch((err: unknown) => setError(`Couldn’t attach: ${err instanceof Error ? err.message : String(err)}`))
  }, [])

  const onDrop = useCallback((e: React.DragEvent) => {
    if (!allowAttachments) return
    const list = e.dataTransfer?.files
    if (!list || list.length === 0) return
    e.preventDefault()
    e.stopPropagation()
    const next: Attachment[] = []
    for (let i = 0; i < list.length; i++) {
      const f = list[i]
      const path = (f as unknown as { path?: string }).path
      next.push(path ? { kind: 'path', path, name: baseName(path) } : { kind: 'file', file: f, name: f.name || 'file' })
    }
    setAttachments((cur) => [...cur, ...next])
  }, [allowAttachments])

  const canSend = !disabled && !sending && (draft.trim().length > 0 || attachments.length > 0)

  return (
    <div
      data-zen-compose=""
      className="flex-shrink-0"
      style={{ padding: '8px var(--zen-gap) var(--zen-gap)' }}
      onDragOver={(e) => {
        if (e.dataTransfer?.types?.includes?.('Files')) e.preventDefault()
      }}
      onDrop={onDrop}
    >
      <ThreadComposeBox
        stacked={stacked}
        chips={
          reply.target || boardContext.context || boardContext.note || attachments.length > 0 ? (
            <>
              {reply.target && <ThreadReplyChip target={reply.target} onCancel={reply.clear} />}
              {boardContext.key && boardContext.context && (
                <ThreadContextComposeChip
                  text={boardContext.context.text}
                  widget={boardContext.context.origin.widget}
                  onRemove={() => removeZenThreadContext(boardContext.key!)}
                />
              )}
              {boardContext.key && !boardContext.context && boardContext.note && (
                <ThreadContextNote note={boardContext.note} onDismiss={() => clearZenThreadContextNote(boardContext.key!)} />
              )}
              {attachments.length > 0 && (
                <ThreadAttachmentChips>
                  {attachments.map((a, i) => (
                    <ThreadAttachmentChip
                      key={`${a.name}-${i}`}
                      name={a.name}
                      onRemove={() => setAttachments((cur) => cur.filter((_, j) => j !== i))}
                    />
                  ))}
                </ThreadAttachmentChips>
              )}
            </>
          ) : undefined
        }
        drawer={drawer}
        error={
          error ? (
            <div data-zen-compose-error="" style={{ marginTop: 6, fontSize: '0.8em', color: 'var(--zen-danger)' }}>
              {error}
            </div>
          ) : undefined
        }
      >
        {allowAttachments && (
          <ThreadRoundButton label="Attach files" disabled={disabled} lightable onClick={pick} data={{ 'data-zen-attach': '' }}>
            +
          </ThreadRoundButton>
        )}
        <textarea
          ref={ref}
          rows={1}
          value={draft}
          disabled={disabled}
          data-zen-compose-input=""
          aria-label={`Message ${label}`}
          placeholder={`Message ${label}…`}
          onChange={(e) => {
            setDraft(e.target.value)
            if (error) setError(null)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault()
              void send()
            } else if (e.key === 'Escape') {
              e.preventDefault()
              // A Reply waiting: Esc cancels it first; then Esc clears.
              if (reply.target) reply.clear()
              else setDraft('')
            }
            // Plain keys stay in the box; ⌘/Ctrl chords reach the app.
            if (!e.metaKey && !e.ctrlKey) e.stopPropagation()
          }}
          className="min-w-0 flex-1 resize-none bg-transparent"
          style={{
            padding: '6px 4px',
            maxHeight: 180,
            color: 'var(--zen-text)',
            font: 'inherit',
            lineHeight: 'var(--zen-line-height)',
            border: 'none',
            ...threadComposeTextSlotStyle(stacked),
          }}
        />
        <ThreadComposeStackSpacer stacked={stacked} />
        <ThreadSendButton canSend={canSend} onClick={() => void send()} />
      </ThreadComposeBox>
    </div>
  )
}

/** Tests only. */
export function __resetZenDraftsForTests(): void {
  __resetZenComposeDraftsForTests()
}
