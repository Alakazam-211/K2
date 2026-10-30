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
  ConnectedStashDrawer,
  isComposeStashChord,
  readLocalComposeFiles,
  useComposePlusMenu,
  useComposeStash,
  useStashDrawerPresence,
  useStashItems,
} from '@/components/PromptStash/composeStash'
import { StashBookmark, stashButtonLabel } from '@/components/PromptStash/PromptStash'
import { insertStashBelow, stashBody, type StashRecord } from '@/lib/prompt-stash/record'
import { readStashFile, usePromptStashStore } from '@/stores/prompt-stash'
import { useToastStore } from '@/stores/toast'
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
    // The "+" is always there (its menu has Save to stash).
    setStacked((cur) => threadComposeStackNext({ stacked: cur, lines, width, rowChrome: zenComposeRowChrome(true) }))
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

  // Save to stash (the "+" menu and Cmd+S, as in the Thread box): the
  // draft and its files, then the box clears. Picked and dropped files are
  // on this computer. A widget holds no server, so the stamp names the agent.
  const stash = useComposeStash({ scope: null, agent: address, agentName: label })
  const plusMenu = useComposePlusMenu()
  const plusRef = useRef<HTMLButtonElement | null>(null)
  const canStash = !disabled && !stash.saving && (draft.trim().length > 0 || attachments.length > 0)
  const stashDraft = useCallback(async () => {
    const startDraft = draft
    const held = attachments
    const paths = held.flatMap((a) => (a.kind === 'path' ? [a.path] : []))
    const blobs = held.flatMap((a) => (a.kind === 'file' ? [{ name: a.name, type: a.file.type, bytes: a.file as Blob }] : []))
    const saved = await stash.save({
      text: startDraft,
      files: async () => [...(paths.length > 0 ? await readLocalComposeFiles(paths) : []), ...blobs],
    })
    if (!saved) return
    // Clear only what was saved: text typed meanwhile stays.
    setDraft((cur) => (cur === startDraft ? '' : cur))
    setAttachments((cur) => cur.filter((a) => !held.includes(a)))
  }, [attachments, draft, setDraft, stash])

  // The stash drawer (the Thread box's): the bookmark left of Send opens
  // it, only while the box is empty; a pick goes below the text, its files
  // attached again.
  const stashDrawer = useStashDrawerPresence()
  const stashCount = useStashItems().length
  const composeEmpty = draft.length === 0 && attachments.length === 0
  const pickStash = useCallback(
    async (rec: StashRecord) => {
      setDraft((cur) => insertStashBelow(cur, stashBody(rec)))
      stashDrawer.hide()
      requestAnimationFrame(() => {
        const ta = ref.current
        if (!ta) return
        ta.focus()
        ta.setSelectionRange(ta.value.length, ta.value.length)
      })
      try {
        if (rec.attachments.length > 0) {
          const read = await Promise.all(rec.attachments.map((a) => readStashFile(a)))
          const files = read.filter((f): f is File => f !== null)
          if (files.length < rec.attachments.length) {
            useToastStore.getState().addToast("Some of this stash's files are gone from this computer.", 'warning')
          }
          if (files.length > 0) setAttachments((cur) => [...cur, ...files.map((file) => ({ kind: 'file' as const, file, name: file.name }))])
        }
        await usePromptStashStore.getState().markUsed(rec.id)
      } catch (err) {
        useToastStore.getState().addToast(`Couldn't restore the stash's files: ${err instanceof Error ? err.message : String(err)}`, 'error')
      }
    },
    [setDraft, stashDrawer],
  )

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
      onKeyDown={(e) => {
        // Cmd+S / Ctrl+S in the box: save to the stash (the box lets
        // chords through to here); on an empty box, open the stash.
        if (!isComposeStashChord(e)) return
        e.preventDefault()
        e.stopPropagation()
        if (canStash) void stashDraft()
        else if (composeEmpty && !stashDrawer.open) stashDrawer.show()
      }}
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
        drawer={
          stashDrawer.open ? (
            // The stash slides up in the drawer's place, over it.
            <div className="grid min-w-0 items-end">
              {drawer ? <div style={{ gridArea: '1 / 1', minWidth: 0 }}>{drawer}</div> : null}
              <div data-zen-stash-slot="" style={{ gridArea: '1 / 1', minWidth: 0, position: 'relative', zIndex: 1, margin: '0 14px -1px' }}>
                <ConnectedStashDrawer
                  look="zen"
                  closing={stashDrawer.closing}
                  chord={stash.chord}
                  onPick={(rec) => void pickStash(rec)}
                  onClose={() => {
                    stashDrawer.hide()
                    ref.current?.focus()
                  }}
                />
              </div>
            </div>
          ) : (
            drawer
          )
        }
        error={
          error ? (
            <div data-zen-compose-error="" style={{ marginTop: 6, fontSize: '0.8em', color: 'var(--zen-danger)' }}>
              {error}
            </div>
          ) : undefined
        }
      >
        <ThreadRoundButton
          label={allowAttachments ? 'Attach or save' : 'Save to stash'}
          title={allowAttachments ? 'Attach files, or save to stash' : 'Save to stash'}
          disabled={disabled}
          lightable
          menuOpen={plusMenu.menuOpen}
          buttonRef={plusRef}
          onClick={() =>
            plusMenu.open(plusRef.current, {
              attach: allowAttachments ? { onPick: pick } : null,
              canStash,
              chord: stash.chord,
              onStash: () => void stashDraft(),
            })
          }
          // `data-zen-attach`: this "+" has an attach choice.
          data={{
            'data-zen-plus': '',
            'data-zen-attach': allowAttachments ? '' : undefined,
            'aria-haspopup': 'menu',
            'aria-expanded': plusMenu.menuOpen,
          }}
        >
          +
        </ThreadRoundButton>
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
        {composeEmpty ? (
          // The stash, left of Send, only while the box is empty.
          <ThreadRoundButton
            label={stashButtonLabel(stashCount)}
            title={`Stash: ${stash.chord} saves what you're writing; ${stash.chord} on an empty box opens this`}
            disabled={disabled}
            pressed={stashDrawer.open && !stashDrawer.closing}
            onClick={stashDrawer.toggle}
            data={{
              'data-testid': 'stash-button',
              'data-stash-button': '',
              'aria-haspopup': 'listbox',
              'aria-expanded': stashDrawer.open && !stashDrawer.closing,
            }}
          >
            <StashBookmark count={stashCount} />
          </ThreadRoundButton>
        ) : null}
        <ThreadSendButton canSend={canSend} onClick={() => void send()} />
      </ThreadComposeBox>
    </div>
  )
}

/** Tests only. */
export function __resetZenDraftsForTests(): void {
  __resetZenComposeDraftsForTests()
}
