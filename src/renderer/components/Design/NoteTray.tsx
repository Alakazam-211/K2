// The notes tray: a DOM strip ABOVE the dock (D-A12). It does not overlap
// the page, so it never freezes it. Numbered like the in-page badges.

import { useState } from 'react'
import { elementChipLabel } from './NotePopover'
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

  return (
    <div className="border-b border-[var(--color-border)] text-xs" data-testid="design-tray">
      <div className="flex items-center gap-2 px-2 h-6">
        <button
          type="button"
          aria-expanded={!collapsed}
          onClick={() => setCollapsed((c) => !c)}
          className="cursor-pointer text-[var(--color-text-primary)]"
        >
          {collapsed ? '▸' : '▾'} Notes ({notes.length})
        </button>
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
      {!collapsed ? (
        <ol className="max-h-40 overflow-y-auto">
          {notes.map((n, i) => {
            const locked = lockedNoteIds.includes(n.id)
            return (
              <li key={n.id} className="flex items-start gap-2 px-2 py-1 border-t border-[var(--color-border)]">
                <span className="font-mono w-4 text-right text-[var(--color-text-muted)]">{i + 1}</span>
                <div className="flex-1 min-w-0">
                  <div className="font-mono text-[10px] text-[var(--color-text-muted)] truncate">
                    {n.intent === 'question' ? 'Question · ' : 'Change · '}
                    {elementChipLabel(n.element)}
                  </div>
                  {editing === n.id ? (
                    <textarea
                      autoFocus
                      aria-label={`Edit note ${i + 1}`}
                      value={draft}
                      rows={2}
                      maxLength={2000}
                      onChange={(e) => setDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Escape') setEditing(null)
                        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && draft.trim()) {
                          onEdit(n.id, draft.trim())
                          setEditing(null)
                        }
                      }}
                      onBlur={() => {
                        if (draft.trim()) onEdit(n.id, draft.trim())
                        setEditing(null)
                      }}
                      className="w-full resize-none p-1 bg-transparent border border-[var(--color-border)]"
                    />
                  ) : (
                    <div className="whitespace-pre-wrap break-words text-[var(--color-text-primary)]">{n.comment}</div>
                  )}
                </div>
                <button
                  type="button"
                  aria-label={`Edit note ${i + 1}`}
                  disabled={sending || locked}
                  onClick={() => {
                    setDraft(n.comment)
                    setEditing(n.id)
                  }}
                  className="px-1 cursor-pointer disabled:opacity-50"
                >
                  Edit
                </button>
                <button
                  type="button"
                  aria-label={`Delete note ${i + 1}`}
                  disabled={sending}
                  onClick={() => onRemove(n.id)}
                  className="px-1 cursor-pointer disabled:opacity-50"
                >
                  Delete
                </button>
              </li>
            )
          })}
        </ol>
      ) : null}
    </div>
  )
}
