// The notes tray: a DOM strip ABOVE the dock (D-A12). It does not overlap
// the page, so it never freezes it. Numbered like the in-page badges.
//
// Rosson 2026-10-16: each note is a tile (chamfered in K2's square Style,
// rounded in the others, `[data-design-tile]` in globals.css) with its
// number and text on one row and an edit pencil and a trash can. Tiles sit
// in a 3-wide by 2-high grid: at most MAX_DESIGN_NOTES (6). Edit is inline.

import { useState } from 'react'
import { PencilIcon, TrashIcon } from '@/components/icons/K2Icons'
import { elementChipLabel } from './NotePopover'
import { MAX_DESIGN_NOTES, NOTES_FULL_REASON } from './useDesignNotes'
import type { DesignNoteDraft } from './types'

export interface NoteTrayProps {
  notes: readonly DesignNoteDraft[]
  sending: boolean
  error: string | null
  /** An undelivered Thread item is waiting: show Retry instead of Send. */
  canRetry: boolean
  /** Notes already inside that item (not editable). */
  lockedNoteIds?: readonly string[]
  onEdit: (noteId: string, comment: string) => void
  onRemove: (noteId: string) => void
  onClear: () => void
  onSend: () => void
  onRetry: () => void
  defaultCollapsed?: boolean
}

const ICON_BUTTON =
  'flex h-5 w-5 flex-shrink-0 items-center justify-center cursor-pointer text-[var(--color-text-secondary)] hover:text-[var(--color-text)] disabled:opacity-40 disabled:cursor-default'

export function NoteTray({
  notes,
  sending,
  error,
  canRetry,
  lockedNoteIds = [],
  onEdit,
  onRemove,
  onClear,
  onSend,
  onRetry,
  defaultCollapsed = false,
}: NoteTrayProps): React.JSX.Element | null {
  const [collapsed, setCollapsed] = useState(defaultCollapsed)
  const [editing, setEditing] = useState<string | null>(null)
  const [draft, setDraft] = useState('')

  if (notes.length === 0 && !error) return null
  const full = notes.length >= MAX_DESIGN_NOTES

  const commit = (noteId: string): void => {
    const text = draft.trim()
    if (text) onEdit(noteId, text)
    setEditing(null)
  }

  return (
    <div className="border-b border-[var(--color-border)] text-xs" data-testid="design-tray">
      <div className="flex items-center gap-2 px-2 h-6">
        <button
          type="button"
          aria-expanded={!collapsed}
          onClick={() => setCollapsed((c) => !c)}
          className="cursor-pointer text-[var(--color-text-primary)]"
        >
          {collapsed ? '▸' : '▾'} Notes ({notes.length}/{MAX_DESIGN_NOTES})
        </button>
        {full ? (
          <span className="min-w-0 truncate text-[10px] text-[var(--color-text-muted)]" data-testid="design-tray-full">
            {NOTES_FULL_REASON}
          </span>
        ) : null}
        <span className="flex-1" />
        <button
          type="button"
          onClick={onClear}
          disabled={sending || notes.length === 0}
          className="px-1 cursor-pointer disabled:opacity-50"
        >
          Clear
        </button>
        {canRetry ? (
          <button type="button" onClick={onRetry} disabled={sending} className="px-2 h-5 border border-[var(--color-border)] cursor-pointer disabled:opacity-50">
            Retry
          </button>
        ) : (
          <button
            type="button"
            onClick={onSend}
            disabled={sending || notes.length === 0}
            className="px-2 h-5 bg-[var(--color-accent)] text-[var(--color-on-accent)] cursor-pointer disabled:opacity-50"
          >
            Send
          </button>
        )}
      </div>
      {error ? (
        <div role="alert" className="px-2 pb-1 text-[var(--color-danger,#c33)]">
          {error}
        </div>
      ) : null}
      {!collapsed && notes.length > 0 ? (
        <ol className="grid grid-cols-3 gap-1.5 px-2 pb-2" data-testid="design-tile-grid">
          {notes.map((n, i) => {
            const locked = lockedNoteIds.includes(n.id)
            const label = `${n.intent === 'question' ? 'Question' : 'Change'} · ${elementChipLabel(n.element)}`
            return (
              <li
                key={n.id}
                data-design-tile=""
                title={`${label}\n${n.comment}`}
                className="flex min-w-0 items-center gap-1.5 h-7 pl-2 pr-1 border border-[var(--color-border)] rounded-[var(--radius-field)] bg-[var(--color-bg-elevated,var(--color-bg))]"
              >
                <span className="font-mono text-[var(--color-accent)] flex-shrink-0" data-testid="design-tile-n">
                  {i + 1}
                </span>
                {editing === n.id ? (
                  <input
                    autoFocus
                    aria-label={`Edit note ${i + 1}`}
                    value={draft}
                    maxLength={2000}
                    onChange={(e) => setDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Escape') {
                        e.preventDefault()
                        setEditing(null)
                      } else if (e.key === 'Enter') {
                        e.preventDefault()
                        commit(n.id)
                      }
                    }}
                    onBlur={() => commit(n.id)}
                    className="flex-1 min-w-0 bg-transparent px-1 border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none"
                  />
                ) : (
                  <span className="flex-1 min-w-0 truncate text-[var(--color-text-primary)]" data-testid="design-tile-text">
                    {n.intent === 'question' ? <span className="text-[var(--color-text-muted)]">? </span> : null}
                    {n.comment}
                  </span>
                )}
                <button
                  type="button"
                  aria-label={`Edit note ${i + 1}`}
                  title="Edit"
                  disabled={sending || locked}
                  onMouseDown={(e) => {
                    // Keep the open editor's blur from committing first.
                    if (editing === n.id) e.preventDefault()
                  }}
                  onClick={() => {
                    if (editing === n.id) {
                      commit(n.id)
                      return
                    }
                    setDraft(n.comment)
                    setEditing(n.id)
                  }}
                  className={ICON_BUTTON}
                >
                  <PencilIcon />
                </button>
                <button
                  type="button"
                  aria-label={`Delete note ${i + 1}`}
                  title="Delete"
                  disabled={sending}
                  onClick={() => {
                    if (editing === n.id) setEditing(null)
                    onRemove(n.id)
                  }}
                  className={ICON_BUTTON}
                >
                  <TrashIcon />
                </button>
              </li>
            )
          })}
        </ol>
      ) : null}
    </div>
  )
}
