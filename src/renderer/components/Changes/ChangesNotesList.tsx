// Changes tab v1.1 (C5): the workspace's review notes in the Changes pane.
// Open notes (unsent first-class, sent tagged, outdated tagged), "Send to
// agent (n)" for the open unsent ones, and resolved notes folded below.
// Data is C2's pane's `useGitNotes` list; this component only renders and
// calls back.

import { useState } from 'react'
import type { NoteCounts, ReviewNote } from '@/stores/git-notes'

export interface ChangesNotesListProps {
  notes: ReviewNote[]
  counts: NoteCounts
  canWrite: boolean
  busy: boolean
  onSend: () => void
  onSelect: (note: ReviewNote) => void
  onResolve: (note: ReviewNote, resolved: boolean) => void
  onEdit: (note: ReviewNote, body: string) => Promise<void>
  onDelete: (note: ReviewNote) => void
}

function where(n: ReviewNote): string {
  const s = n.currentStartLine ?? n.startLine
  const e = n.currentEndLine ?? n.endLine
  return `${n.path}:${s === e ? s : `${s}–${e}`}${n.side === 'old' ? ' (before)' : ''}`
}

function NoteRow(props: {
  note: ReviewNote
  canWrite: boolean
  busy: boolean
  onSelect: () => void
  onResolve: (resolved: boolean) => void
  onEdit: (body: string) => Promise<void>
  onDelete: () => void
}): React.JSX.Element {
  const { note } = props
  const [editing, setEditing] = useState<string | null>(null)
  const resolved = note.resolvedAt != null
  const link = 'text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-40'
  return (
    <li
      data-note-row={note.id}
      data-outdated={note.outdated && !resolved ? '' : undefined}
      className="border-b border-[var(--color-border)] px-2 py-1.5 text-xs"
    >
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          className="min-w-0 truncate text-left text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
          title={`Show ${note.path}`}
          onClick={props.onSelect}
        >
          {where(note)}
        </button>
        {note.sentAt != null && !resolved && (
          <span className="flex-shrink-0 border border-[var(--color-border)] px-1 text-[9px] text-[var(--color-text-muted)]">sent</span>
        )}
        {note.outdated && !resolved && (
          <span data-note-outdated="" className="flex-shrink-0 border border-[var(--color-border)] px-1 text-[9px] text-[var(--color-status-warn-text)]">
            outdated
          </span>
        )}
        <span className="flex-1" />
        <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">{note.authorName}</span>
      </div>
      {editing === null ? (
        <p className="mt-0.5 line-clamp-3 whitespace-pre-wrap break-words text-[var(--color-text-primary)]">{note.body}</p>
      ) : (
        <textarea
          aria-label="Edit note"
          className="mt-1 w-full resize-none border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-1.5 py-1 text-xs text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]"
          rows={3}
          value={editing}
          autoFocus
          onChange={(e) => setEditing(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') setEditing(null)
            if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
              e.preventDefault()
              const text = editing.trim()
              if (text) void props.onEdit(text).then(() => setEditing(null))
            }
          }}
        />
      )}
      {props.canWrite && (
        <div className="mt-1 flex gap-2">
          {editing !== null ? (
            <>
              <button
                type="button"
                className={link}
                disabled={props.busy || !editing.trim()}
                onClick={() => void props.onEdit(editing.trim()).then(() => setEditing(null))}
              >
                Save
              </button>
              <button type="button" className={link} onClick={() => setEditing(null)}>
                Cancel
              </button>
            </>
          ) : (
            <>
              <button type="button" className={link} disabled={props.busy} onClick={() => props.onResolve(!resolved)}>
                {resolved ? 'Reopen' : 'Resolve'}
              </button>
              {!resolved && (
                <button type="button" className={link} disabled={props.busy} onClick={() => setEditing(note.body)}>
                  Edit
                </button>
              )}
              <button type="button" className={link} disabled={props.busy} onClick={props.onDelete}>
                Delete
              </button>
            </>
          )}
        </div>
      )}
    </li>
  )
}

export function ChangesNotesList(props: ChangesNotesListProps): React.JSX.Element | null {
  const [showResolved, setShowResolved] = useState(false)
  const open = props.notes.filter((n) => n.resolvedAt == null)
  const resolved = props.notes.filter((n) => n.resolvedAt != null)
  if (open.length === 0 && resolved.length === 0) return null
  const row = (n: ReviewNote): React.JSX.Element => (
    <NoteRow
      key={n.id}
      note={n}
      canWrite={props.canWrite}
      busy={props.busy}
      onSelect={() => props.onSelect(n)}
      onResolve={(r) => props.onResolve(n, r)}
      onEdit={(b) => props.onEdit(n, b)}
      onDelete={() => props.onDelete(n)}
    />
  )
  return (
    <section data-changes-notes="" className="border-t border-[var(--color-border)]">
      <div className="flex items-center gap-2 px-2 py-1">
        <span className="text-[11px] font-medium text-[var(--color-text-primary)]">Notes</span>
        <span className="text-[10px] tabular-nums text-[var(--color-text-muted)]">{props.counts.open} open</span>
        <span className="flex-1" />
        {props.canWrite && (
          <button
            type="button"
            data-notes-send=""
            className="bg-[var(--color-accent)] px-2 py-0.5 text-[10px] font-medium text-[var(--color-on-accent)] hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
            disabled={props.busy || props.counts.unsent === 0}
            title="Send the open notes that have not been sent to this workspace's agent, as one message"
            onClick={props.onSend}
          >
            Send to agent ({props.counts.unsent})
          </button>
        )}
      </div>
      <ul>{open.map(row)}</ul>
      {resolved.length > 0 && (
        <>
          <button
            type="button"
            data-notes-resolved-toggle=""
            className="w-full px-2 py-1 text-left text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
            aria-expanded={showResolved}
            onClick={() => setShowResolved((s) => !s)}
          >
            {showResolved ? '▾' : '▸'} Resolved ({resolved.length})
          </button>
          {showResolved && <ul className="opacity-70">{resolved.map(row)}</ul>}
        </>
      )}
    </section>
  )
}
