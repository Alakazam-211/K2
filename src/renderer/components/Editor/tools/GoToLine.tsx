// Go to line (build-editors.md E3): ⌘L, or the status line's Ln, Col. A small
// field over the status line's left end: "12" or "L12", a column after it if
// you like ("12:4", "L12:C4"). Enter goes there and puts the keyboard back in
// the text; a line past the end says how many there are; Esc closes it.

import { useLayoutEffect, useRef, useState } from 'react'
import { EditorSelection } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { floatMaterial } from '@/components/ui'
import { parseLineTarget } from '@/lib/editor/goto-line'

export function goToLine(view: EditorView, line: number, col: number | null): void {
  const l = view.state.doc.line(line)
  const pos = l.from + Math.min(Math.max(0, (col ?? 1) - 1), l.length)
  view.dispatch({ selection: EditorSelection.cursor(pos), effects: EditorView.scrollIntoView(pos, { y: 'center' }), userEvent: 'select.goto' })
  view.focus()
}

export function GoToLine({ view, onClose }: { view: EditorView; onClose: () => void }): React.JSX.Element {
  const [text, setText] = useState('')
  const [error, setError] = useState<string | null>(null)
  const field = useRef<HTMLInputElement>(null)
  useLayoutEffect(() => field.current?.focus(), [])
  const lines = view.state.doc.lines
  const go = (): void => {
    const t = parseLineTarget(text, lines)
    if (!t) return
    if ('error' in t) return setError(t.error)
    goToLine(view, t.line, t.col)
    onClose()
  }
  return (
    <div data-testid="ed-goto" data-k2-chamfer="control" className="ed-goto" {...floatMaterial('popover', 'frost')}>
      <label className="ed-goto-row">
        <span className="ed-goto-label">Go to line</span>
        <input
          ref={field}
          type="text"
          inputMode="numeric"
          aria-label="Go to line"
          aria-describedby={error ? 'ed-goto-error' : 'ed-goto-hint'}
          aria-invalid={error ? true : undefined}
          placeholder={`1–${lines}`}
          spellCheck={false}
          autoComplete="off"
          data-k2-chamfer="control"
          className="ed-fld ed-goto-fld no-drag"
          value={text}
          onChange={(e) => {
            setText(e.target.value)
            setError(null)
          }}
          onBlur={(e) => {
            if (!e.currentTarget.closest('[data-testid="ed-goto"]')?.contains(e.relatedTarget as Node | null)) onClose()
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              go()
            } else if (e.key === 'Escape') {
              e.preventDefault()
              e.stopPropagation()
              onClose()
              view.focus()
            }
          }}
        />
      </label>
      {error ? (
        <p id="ed-goto-error" data-testid="ed-goto-error" role="alert" className="ed-goto-note ed-goto-bad">
          {error}
        </p>
      ) : (
        <p id="ed-goto-hint" className="ed-goto-note">
          A line, or L12:4 for a column
        </p>
      )}
    </div>
  )
}
