// The editor's status line (build-editors.md E1b; the notes' §8): quiet,
// under the text. Markdown: N words · N min read · the save word (Source
// adds Ln, Col). Code: Ln, Col · the save word, then language, encoding and
// line endings. The view's toggles (Full width, Wrap) sit on the right; in a
// narrow tab they, and code's file facts, fold behind one Display button
// that opens a small group above it.

import { Fragment, useEffect, useRef, useState } from 'react'

export interface StatusToggle {
  id: string
  label: string
  on: boolean
  onToggle: () => void
}

export interface StatusItem {
  id: string
  node: React.ReactNode
}

function Check(): React.JSX.Element {
  return (
    <svg aria-hidden="true" width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M3 8.5 6.5 12 13 4.5" />
    </svg>
  )
}

/** "Saved" with a tick, or "Not saved" / "Saving" with a dot. */
export function SaveWord({ word }: { word: 'Saved' | 'Not saved' | 'Saving' }): React.JSX.Element {
  return (
    <>
      {word === 'Saved' ? (
        <span className="ed-ok inline-flex">
          <Check />
        </span>
      ) : (
        <span aria-hidden="true" className="ed-unsaved-dot" />
      )}
      {word}
    </>
  )
}

function Toggles({ toggles }: { toggles: StatusToggle[] }): React.JSX.Element {
  return (
    <>
      {toggles.map((t) => (
        <button key={t.id} type="button" data-k2-chamfer="chip" aria-pressed={t.on} title={t.label} onClick={t.onToggle} className="ed-tog no-drag">
          {t.on ? <Check /> : null}
          {t.label}
        </button>
      ))}
    </>
  )
}

function Facts({ facts }: { facts: StatusItem[] }): React.JSX.Element | null {
  if (facts.length === 0) return null
  return (
    <span className="ed-facts">
      {facts.map((f) => (
        <span key={f.id} data-testid={`ed-stat-${f.id}`} className="ed-si">
          {f.node}
        </span>
      ))}
    </span>
  )
}

export function EditorStatusLine({ items, toggles, facts, narrow }: { items: StatusItem[]; toggles: StatusToggle[]; facts: StatusItem[]; narrow: boolean }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const group = useRef<HTMLDivElement>(null)
  const button = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (!narrow) setOpen(false)
  }, [narrow])
  useEffect(() => {
    if (!open) return
    group.current?.querySelector<HTMLElement>('button')?.focus()
    const away = (e: MouseEvent): void => {
      const t = e.target as Node
      if (!group.current?.contains(t) && !button.current?.contains(t)) setOpen(false)
    }
    document.addEventListener('mousedown', away)
    return () => document.removeEventListener('mousedown', away)
  }, [open])

  const folds = narrow && (toggles.length > 0 || facts.length > 0)
  return (
    <div data-testid="editor-status" className="ed-status">
      {items.map((it, i) => (
        <Fragment key={it.id}>
          {i > 0 ? (
            <span aria-hidden="true" className="ed-sdot">
              ·
            </span>
          ) : null}
          <span data-testid={`ed-stat-${it.id}`} className="ed-si">
            {it.node}
          </span>
        </Fragment>
      ))}
      <span className="flex-1" />
      {folds ? (
        <>
          <button ref={button} type="button" data-k2-chamfer="chip" aria-haspopup="true" aria-expanded={open} onClick={() => setOpen((o) => !o)} className="ed-tog no-drag">
            <svg aria-hidden="true" width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
              <path d="M3 4h10M3 8h10M3 12h10" />
            </svg>
            Display
          </button>
          {open ? (
            <div
              ref={group}
              role="group"
              aria-label="Display"
              data-k2-chamfer="control"
              className="ed-display"
              onKeyDown={(e) => {
                if (e.key !== 'Escape') return
                e.stopPropagation()
                setOpen(false)
                button.current?.focus()
              }}
            >
              <Toggles toggles={toggles} />
              <Facts facts={facts} />
            </div>
          ) : null}
        </>
      ) : (
        <>
          {toggles.length ? (
            <span className="ed-togs">
              <Toggles toggles={toggles} />
            </span>
          ) : null}
          <Facts facts={facts} />
        </>
      )}
    </div>
  )
}
