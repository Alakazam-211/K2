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

type Attachment = { kind: 'path'; path: string; name: string } | { kind: 'file'; file: File; name: string }

const baseName = (p: string): string => p.split(/[/\\]/).pop() || p

export function ZenCompose({
  bridge,
  address,
  label,
  disabled,
  attachments: allowAttachments = true,
  onSend,
}: {
  bridge: ZenWidgetBridge
  address: string
  label: string
  disabled: boolean
  /** The `attachments` prop: "+" and drops (default on). */
  attachments?: boolean
  /** A send started (the conversation scrolls to its bottom). */
  onSend?: () => void
}): React.JSX.Element {
  const [draft, setDraftState] = useState(() => zenComposeDraft(address))
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const ref = useRef<HTMLTextAreaElement | null>(null)

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
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const row = el.parentElement
    const held = row ? row.style.minHeight : ''
    if (row) row.style.minHeight = `${row.offsetHeight}px`
    el.style.height = 'auto'
    const next = `${Math.min(el.scrollHeight, 180)}px`
    el.style.height = next
    if (row) row.style.minHeight = held
  }, [draft])

  const send = useCallback(async () => {
    if (disabled || sending) return
    const text = draft.trim()
    if (!text && attachments.length === 0) return
    const paths = attachments.flatMap((a) => (a.kind === 'path' ? [a.path] : []))
    const files = attachments.flatMap((a) => (a.kind === 'file' ? [a.file] : []))
    const held = attachments
    onSend?.()
    setSending(true)
    setError(null)
    setDraft('')
    setAttachments([])
    try {
      await (bridge.call('thread.post', address, text, {
        ...(paths.length > 0 ? { paths } : {}),
        ...(files.length > 0 ? { files } : {}),
      }) as Promise<unknown>)
    } catch (err) {
      setDraft((cur) => (cur.length === 0 ? text : cur))
      setAttachments((cur) => (cur.length === 0 ? held : cur))
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSending(false)
    }
  }, [address, attachments, bridge, disabled, draft, onSend, sending, setDraft])

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
      {attachments.length > 0 && (
        <div className="flex flex-wrap gap-1.5" style={{ marginBottom: 6 }} data-zen-attachments="">
          {attachments.map((a, i) => (
            <span
              key={`${a.name}-${i}`}
              data-zen-attachment={a.name}
              className="inline-flex max-w-[16rem] items-center gap-1.5"
              style={{
                fontSize: '0.8em',
                padding: '3px 4px 3px 10px',
                borderRadius: 999,
                background: 'var(--zen-surface-raised)',
                border: '1px solid var(--zen-border)',
                color: 'var(--zen-text)',
              }}
            >
              <span className="truncate">{a.name}</span>
              <button
                type="button"
                aria-label={`Remove ${a.name}`}
                data-zen-soft-button=""
                onClick={() => setAttachments((cur) => cur.filter((_, j) => j !== i))}
                className="flex items-center justify-center cursor-pointer disabled:cursor-default"
                style={{ width: 18, height: 18, borderRadius: 999, color: 'var(--zen-text-muted)' }}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      <div
        className="flex items-end gap-2"
        style={{
          padding: 6,
          borderRadius: 'calc(var(--zen-radius) + 4px)',
          background: 'var(--zen-surface-raised)',
          border: '1px solid var(--zen-border)',
        }}
      >
        {allowAttachments && (
        <button
          type="button"
          aria-label="Attach files"
          title="Attach files"
          data-zen-attach=""
          data-zen-soft-button=""
          disabled={disabled}
          onClick={pick}
          className="flex flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default"
          style={{ width: 32, height: 32, borderRadius: 999, color: 'var(--zen-text-muted)', fontSize: 20, lineHeight: 1 }}
        >
          +
        </button>
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
              setDraft('')
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
          }}
        />
        <button
          type="button"
          aria-label="Send"
          title="Send (Enter)"
          data-zen-send=""
          disabled={!canSend}
          onClick={() => void send()}
          className="flex flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default"
          style={{
            width: 32,
            height: 32,
            borderRadius: 999,
            background: canSend ? 'var(--zen-accent)' : 'var(--zen-surface)',
            color: canSend ? 'var(--zen-accent-text)' : 'var(--zen-text-muted)',
            transition: 'background-color 140ms ease',
          }}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="M8 13V3" />
            <path d="M3.5 7.5 8 3l4.5 4.5" />
          </svg>
        </button>
      </div>
      {error && (
        <div data-zen-compose-error="" style={{ marginTop: 6, fontSize: '0.8em', color: 'var(--zen-danger)' }}>
          {error}
        </div>
      )}
    </div>
  )
}

/** Tests only. */
export function __resetZenDraftsForTests(): void {
  __resetZenComposeDraftsForTests()
}
