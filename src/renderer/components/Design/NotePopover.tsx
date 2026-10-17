// The note editor beside a picked element (PRD C1, D-A12; reworked Rosson
// 2026-10-16).
//
// A tile titled "New Note" (the notes section's header style), the picked
// element's chip, Change | Question tabs (the note's kind), the text, and
// three actions: Cancel, Add (pin it in the tray, send later) and Send now
// (just this note, straight away, with its own details file). ⌘↩
// (Ctrl+Enter off macOS) is Send now. Plain Return is a new line.
//
// It claims its own box through the Browser overlay arbiter. The box
// overlaps the dock, so the arbiter freezes the page (its snapshot replaces
// the native view) while the person types: the page never sees or forges the
// note text. Closing the pop-up releases the claim and the page goes live.

import { useLayoutEffect, useRef, useState } from 'react'
import { NavSectionLabel } from '@/components/Sidebar/NavSection'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { placePopover, type PopoverPlacement } from './placement'
import type { NoteElement, NoteIntent, NoteRect } from './types'

export interface NotePopoverProps {
  open: boolean
  /** The picked element (absent: a whole-page note). */
  element?: NoteElement
  /** The element's box in window CSS px (see `scaleGuestRect`). */
  anchor: NoteRect
  /** Where the pop-up may sit (the Design pane), window CSS px. */
  container: NoteRect
  initialComment?: string
  initialIntent?: NoteIntent
  /** Add: pin the note in the tray. */
  onAdd: (comment: string, intent: NoteIntent) => void
  /** Send now (and ⌘↩): just this note, straight away. */
  onSendNow?: (comment: string, intent: NoteIntent) => void
  /** A brief notice (the tray is full after an Add), or null. */
  notice?: string | null
  onCancel: () => void
}

/** Starting size before the first measure. */
const DEFAULT_SIZE = { width: 320, height: 200 }

/** The macOS system font, so ⌘ and ↩ draw as the system draws them (the
 *  app's mono font has no good glyphs for them). */
export const SYSTEM_KEY_FONT = '-apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro", system-ui, sans-serif'

/** The send-now chord on this platform. */
export function sendNowChord(mac: boolean = isMacPlatform()): string {
  return mac ? '⌘↩' : 'Ctrl+Enter'
}

/** The chord as a small key cap in the system font. */
export function ShortcutKbd({ chord, className = '' }: { chord: string; className?: string }): React.JSX.Element {
  return (
    <kbd
      data-testid="design-shortcut"
      className={`inline-block text-[10px] leading-none whitespace-nowrap opacity-75 ${className}`}
      style={{ fontFamily: SYSTEM_KEY_FONT }}
    >
      {chord}
    </kbd>
  )
}

export function elementChipLabel(e: NoteElement | undefined): string {
  if (!e) return 'Whole page'
  let s = e.tag || 'element'
  if (e.id) s += `#${e.id}`
  for (const c of (e.classes ?? []).slice(0, 2)) s += `.${c}`
  return `<${s}>`
}

const KINDS: readonly NoteIntent[] = ['change', 'question']
const KIND_LABEL: Record<NoteIntent, string> = { change: 'Change', question: 'Question' }

export function NotePopover({
  open,
  element,
  anchor,
  container,
  initialComment = '',
  initialIntent = 'change',
  onAdd,
  onSendNow,
  notice = null,
  onCancel,
}: NotePopoverProps): React.JSX.Element | null {
  const ref = useRef<HTMLDivElement | null>(null)
  const tabRefs = useRef<Record<NoteIntent, HTMLButtonElement | null>>({ change: null, question: null })
  const [comment, setComment] = useState(initialComment)
  const [intent, setIntent] = useState<NoteIntent>(initialIntent)
  const [size, setSize] = useState(DEFAULT_SIZE)

  useBrowserOverlay({ active: open, ref, kind: 'popover' })

  useLayoutEffect(() => {
    const el = ref.current
    if (!open || !el) return
    const w = el.offsetWidth
    const h = el.offsetHeight
    if (w > 0 && h > 0 && (w !== size.width || h !== size.height)) setSize({ width: w, height: h })
  }, [open, size.width, size.height])

  if (!open) return null

  const place: PopoverPlacement = placePopover(anchor, size, container)
  const trimmed = comment.trim()
  const add = (): void => {
    if (!trimmed) return
    onAdd(trimmed, intent)
  }
  const canSendNow = !!onSendNow && trimmed.length > 0
  const sendNow = (): void => {
    if (!canSendNow || !onSendNow) return
    onSendNow(trimmed, intent)
  }
  const chord = sendNowChord()
  const pickKind = (k: NoteIntent): void => {
    setIntent(k)
    tabRefs.current[k]?.focus()
  }

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label="Note for the agent"
      data-side={place.side}
      className="fixed z-50 w-80 shadow-lg text-xs no-drag"
      style={{ left: place.x, top: place.y }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          onCancel()
        } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
          e.preventDefault()
          sendNow()
        }
      }}
    >
      <div
        data-design-tile=""
        className="flex flex-col border border-[var(--color-border)] rounded-[var(--radius-field)] bg-[var(--color-bg-elevated,var(--color-bg))]"
      >
        <div className="flex items-center gap-1.5 px-3 py-1.5" data-testid="design-new-note-header">
          <NavSectionLabel>New Note</NavSectionLabel>
          <span className="min-w-0 flex-1 truncate text-right font-mono text-[10px] text-[var(--color-text-muted)]" data-testid="design-element-chip">
            {elementChipLabel(element)}
          </span>
        </div>
        <div role="tablist" aria-label="Kind of note" className="flex items-center gap-1 px-3 border-b border-[var(--color-border)]">
          {KINDS.map((k, i) => (
            <button
              key={k}
              ref={(el) => {
                tabRefs.current[k] = el
              }}
              type="button"
              role="tab"
              aria-selected={intent === k}
              tabIndex={intent === k ? 0 : -1}
              onClick={() => setIntent(k)}
              onKeyDown={(e) => {
                if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
                  e.preventDefault()
                  const next = KINDS[(i + (e.key === 'ArrowRight' ? 1 : KINDS.length - 1)) % KINDS.length]
                  pickKind(next)
                } else if (e.key === 'Home' || e.key === 'End') {
                  e.preventDefault()
                  pickKind(e.key === 'Home' ? KINDS[0] : KINDS[KINDS.length - 1])
                }
              }}
              className={`px-3 py-1.5 text-[11px] font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
                intent === k
                  ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                  : 'border-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
            >
              {KIND_LABEL[k]}
            </button>
          ))}
        </div>
        <div className="flex flex-col gap-1.5 p-3" role="tabpanel" aria-label={KIND_LABEL[intent]}>
          <textarea
            autoFocus
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            placeholder={intent === 'question' ? 'Ask about this…' : 'What should change?'}
            rows={3}
            maxLength={2000}
            className="w-full resize-none p-1 bg-transparent border border-[var(--color-border)] text-[var(--color-text-primary)]"
          />
          {notice ? (
            <div role="status" className="text-[10px] text-[var(--color-text-secondary)]" data-testid="design-popover-notice">
              {notice}
            </div>
          ) : null}
          <div className="flex items-center gap-1.5">
            <button type="button" onClick={onCancel} className="px-2 h-6 cursor-pointer text-[var(--color-text-secondary)] hover:text-[var(--color-text)]">
              Cancel
            </button>
            <span className="flex-1" />
            <button
              type="button"
              onClick={add}
              disabled={!trimmed}
              title="Pin this note; send it with the others later"
              className="px-2 h-6 border border-[var(--color-border)] text-[var(--color-text-primary)] cursor-pointer disabled:opacity-50"
            >
              Add
            </button>
            {onSendNow ? (
              <button
                type="button"
                onClick={sendNow}
                disabled={!canSendNow}
                aria-label="Send now"
                title={`Send just this note now (${chord})`}
                className="flex items-center gap-1.5 px-2 h-6 bg-[var(--color-accent)] text-[var(--color-on-accent)] cursor-pointer disabled:opacity-50"
              >
                <span>Send now</span>
                <ShortcutKbd chord={chord} />
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  )
}
