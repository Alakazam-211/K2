// The formatting toolbar for Write (build-editors.md E3; editors-notes.md §2,
// v2 "nothing covers text"): Bold, Italic, Code, Link, then Heading and List.
// A WAI-ARIA toolbar: one tab stop (roving tabindex), ← → move and wrap, Home
// and End, Esc back to the text. Each button names itself, shows its key in
// the tooltip (and aria-keyshortcuts), and is on where the caret is in it.
// Where it sits (the right margin beside the selection, or docked in the file
// bar when there's no margin) is the frame's (FrameTools).

import { forwardRef, useState } from 'react'
import { FORMAT_IDS, FORMAT_KEYS, FORMAT_NAMES, type FormatId, type FormatState } from '@/lib/editor/md-commands'
import { floatMaterial } from '@/components/ui'
import { ToolGlyph } from './glyphs'

export interface FormatToolbarProps {
  active: FormatState
  onRun: (id: FormatId) => void
  /** Esc: back to the text. */
  onEscape: () => void
  /** In the file bar (narrow), not floating. */
  docked?: boolean
  style?: React.CSSProperties
}

export const FormatToolbar = forwardRef<HTMLDivElement, FormatToolbarProps>(function FormatToolbar({ active, onRun, onEscape, docked = false, style }, ref) {
  const [stop, setStop] = useState(0)
  const go = (e: React.KeyboardEvent, i: number): void => {
    const n = FORMAT_IDS.length
    const next = e.key === 'ArrowRight' ? (i + 1) % n : e.key === 'ArrowLeft' ? (i - 1 + n) % n : e.key === 'Home' ? 0 : e.key === 'End' ? n - 1 : -1
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      onEscape()
      return
    }
    if (next < 0) return
    e.preventDefault()
    setStop(next)
    const bar = (e.currentTarget as HTMLElement).closest('[role="toolbar"]')
    bar?.querySelectorAll<HTMLButtonElement>('button')[next]?.focus()
  }
  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label="Format the selection"
      aria-orientation="horizontal"
      data-testid="ed-format"
      data-docked={docked ? '' : undefined}
      data-k2-chamfer="control"
      className="ed-fmt no-drag"
      style={style}
      // Keep the selection: a press on the toolbar never takes the focus from the text.
      onMouseDown={(e) => e.preventDefault()}
      {...(docked ? {} : floatMaterial('menu', 'frost'))}
    >
      {FORMAT_IDS.map((id, i) => (
        <span key={id} className="contents">
          {id === 'heading' ? <span aria-hidden="true" className="ed-fsep" /> : null}
          <button
            type="button"
            data-k2-focus=""
            data-k2-chamfer="control"
            tabIndex={i === stop ? 0 : -1}
            aria-label={FORMAT_NAMES[id]}
            aria-pressed={active[id]}
            aria-keyshortcuts={FORMAT_KEYS[id]?.aria}
            title={`${FORMAT_NAMES[id]} (${FORMAT_KEYS[id]?.label ?? 'or paste a URL over the words'})`}
            className="ed-fb"
            onFocus={() => setStop(i)}
            onKeyDown={(e) => go(e, i)}
            onClick={() => onRun(id)}
          >
            <ToolGlyph name={id} />
          </button>
        </span>
      ))}
    </div>
  )
})
