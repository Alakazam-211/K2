// The note pop-up beside a picked element (PRD C1, D-A12).
//
// It claims its own box through the Browser overlay arbiter. The box
// overlaps the dock, so the arbiter freezes the page (its snapshot replaces
// the native view) while the person types: the page never sees or forges the
// note text. No freeze code lives here. Closing the pop-up releases the
// claim and the page goes live again.

import { useLayoutEffect, useRef, useState } from 'react'
import { useBrowserOverlay } from '@/lib/browser-overlay'
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
  onAdd: (comment: string, intent: NoteIntent) => void
  onCancel: () => void
}

/** Starting size before the first measure. */
const DEFAULT_SIZE = { width: 320, height: 180 }

export function elementChipLabel(e: NoteElement | undefined): string {
  if (!e) return 'Whole page'
  let s = e.tag || 'element'
  if (e.id) s += `#${e.id}`
  for (const c of (e.classes ?? []).slice(0, 2)) s += `.${c}`
  return `<${s}>`
}

export function NotePopover({
  open,
  element,
  anchor,
  container,
  initialComment = '',
  initialIntent = 'change',
  onAdd,
  onCancel,
}: NotePopoverProps): React.JSX.Element | null {
  const ref = useRef<HTMLDivElement | null>(null)
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

  return (
    <div
      ref={ref}
      role="dialog"
      aria-label="Note for the agent"
      data-side={place.side}
      className="fixed z-50 w-80 p-2 flex flex-col gap-2 bg-[var(--color-bg-elevated,var(--color-bg))] border border-[var(--color-border)] shadow-lg text-xs no-drag"
      style={{ left: place.x, top: place.y }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
          onCancel()
        } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
          e.preventDefault()
          add()
        }
      }}
    >
      <div className="font-mono text-[10px] text-[var(--color-text-muted)] truncate" data-testid="design-element-chip">
        {elementChipLabel(element)}
      </div>
      <textarea
        autoFocus
        value={comment}
        onChange={(e) => setComment(e.target.value)}
        placeholder={intent === 'question' ? 'Ask about this…' : 'What should change?'}
        rows={3}
        maxLength={2000}
        className="w-full resize-none p-1 bg-transparent border border-[var(--color-border)] text-[var(--color-text-primary)]"
      />
      <div className="flex items-center gap-1">
        <div role="radiogroup" aria-label="Kind of note" className="flex">
          {(['change', 'question'] as const).map((k) => (
            <button
              key={k}
              type="button"
              role="radio"
              aria-checked={intent === k}
              onClick={() => setIntent(k)}
              className={`px-2 h-5 cursor-pointer ${
                intent === k
                  ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                  : 'border border-[var(--color-border)]'
              }`}
            >
              {k === 'change' ? 'Change' : 'Question'}
            </button>
          ))}
        </div>
        <span className="flex-1" />
        <button type="button" onClick={onCancel} className="px-2 h-5 cursor-pointer">
          Cancel
        </button>
        <button
          type="button"
          onClick={add}
          disabled={!trimmed}
          title="Add (⌘↩)"
          className="px-2 h-5 bg-[var(--color-accent)] text-[var(--color-on-accent)] cursor-pointer disabled:opacity-50"
        >
          Add
        </button>
      </div>
    </div>
  )
}
