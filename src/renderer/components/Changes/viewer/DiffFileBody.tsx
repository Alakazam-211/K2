// One file's diff body: loading / error / fallback cards, or the
// CodeMirror merge editor for text.

import { useMemo } from 'react'
import type { DiffContent, DiffView } from './diffQuery'
import { formatBytes } from './diffQuery'
import { MergeDiffEditor, type EditorNotes, type MergeDiffApi } from './MergeDiffEditor'
import { placeNotes } from './notesPlacement'
import type { ChangesNotesApi } from '../diff-slot-types'
import type { ChangesDiffFile, ChangesDiffLayout } from './types'

const STATUS_LABEL: Record<string, string> = {
  M: 'Modified',
  A: 'Added',
  D: 'Deleted',
  R: 'Renamed',
  T: 'Type changed',
  U: 'Untracked',
  C: 'Conflicted',
}

export function statusLabel(status: string): string {
  return STATUS_LABEL[status] ?? status
}

const STATUS_COLOR: Record<string, string> = {
  M: 'var(--color-accent)',
  A: 'var(--color-status-ok)',
  U: 'var(--color-status-ok)',
  D: 'var(--color-status-error)',
  C: 'var(--color-status-error)',
  R: 'var(--color-accent)',
  T: 'var(--color-accent)',
}

export function StatusBadge({ status }: { status: string }): React.JSX.Element {
  return (
    <span
      className="inline-flex items-center justify-center w-4 h-4 text-[10px] font-semibold shrink-0"
      style={{ color: STATUS_COLOR[status] ?? 'var(--color-text-muted)' }}
      title={statusLabel(status)}
    >
      {status}
    </span>
  )
}

function Card({ title, children, testId }: { title: string; children?: React.ReactNode; testId: string }): React.JSX.Element {
  return (
    <div className="flex items-center justify-center p-6 min-h-[96px]" data-testid={testId}>
      <div className="max-w-md text-center">
        <div className="text-xs font-medium text-[var(--color-text-primary)]">{title}</div>
        {children && <div className="mt-1 text-xs text-[var(--color-text-muted)]">{children}</div>}
      </div>
    </div>
  )
}

function sizes(content: { oldSize: number; newSize: number; change: string }): string {
  if (content.change === 'added') return formatBytes(content.newSize)
  if (content.change === 'deleted') return formatBytes(content.oldSize)
  return `${formatBytes(content.oldSize)} → ${formatBytes(content.newSize)}`
}

export interface DiffFileBodyProps {
  row: ChangesDiffFile
  /** `reload` (when given) adds a Retry button to the error card. */
  state: DiffView & { reload?: () => void }
  layout: ChangesDiffLayout
  wrap: boolean
  collapse: boolean
  fill: boolean
  onOpenFile?: (repoRelPath: string) => void
  onChunks?: (starts: number[]) => void
  onCursor?: (pos: number) => void
  apiRef?: React.MutableRefObject<MergeDiffApi | null>
  /** Review notes (C5). */
  notes?: ChangesNotesApi
  /** The snapshot rev a new note is made on. */
  rev?: string | null
}

/** The editor's notes for a text diff of `row` (null without notes). */
function useEditorNotes(
  row: ChangesDiffFile,
  content: DiffContent | null,
  layout: ChangesDiffLayout,
  notes: ChangesNotesApi | undefined,
  rev: string | null | undefined,
): EditorNotes | undefined {
  const text = content?.kind === 'text' ? content : null
  const list = notes?.notes
  const canWrite = notes?.canWrite ?? false
  const marks = useMemo(() => {
    if (!text || !list) return null
    return {
      new: placeNotes(list, row.path, 'new', text.newText),
      // Inline shows only the new text; old-side notes live in the list.
      old: layout === 'split' ? placeNotes(list, row.oldPath ?? row.path, 'old', text.oldText) : [],
    }
  }, [text, list, row.path, row.oldPath, layout])
  if (!marks || !notes) return undefined
  return {
    marks,
    canWrite,
    handlers: (side) => ({
      canWrite,
      save: (from, to, body, anchor) =>
        notes.add({
          filePath: side === 'old' && row.oldPath ? row.oldPath : row.path,
          side,
          startLine: from,
          endLine: to,
          body,
          anchorText: anchor,
          rev: rev ?? null,
        }),
      edit: (id, body) => notes.update(id, body),
      resolve: (id) => notes.resolve(id),
      remove: (id) => notes.remove(id),
    }),
  }
}

export function DiffFileBody(props: DiffFileBodyProps): React.JSX.Element {
  const { row, state, onOpenFile } = props
  const editorNotes = useEditorNotes(row, state.content, props.layout, props.notes, props.rev)
  if (state.error) {
    return (
      <Card title="Couldn't load this diff" testId="diff-error">
        <span className="break-words">{state.error}</span>
        {state.reload && (
          <>
            {' '}
            <button className="underline hover:text-[var(--color-text-primary)] cursor-pointer" onClick={state.reload}>
              Retry
            </button>
          </>
        )}
      </Card>
    )
  }
  const content: DiffContent | null = state.content
  if (!content) {
    return <Card title="Loading diff…" testId="diff-loading" />
  }
  switch (content.kind) {
    case 'dir':
      return (
        <Card title="New folder" testId="diff-dir">
          This untracked folder is listed as one entry. Stage it or open its files to see them.
        </Card>
      )
    case 'binary':
      return (
        <Card title="Binary file" testId="diff-binary">
          Not shown as text ({sizes(content)}).
        </Card>
      )
    case 'tooLarge':
      return (
        <Card title="This diff is too large to show" testId="diff-too-large">
          {sizes(content)}, over the 10 MB or 120,000-line limit.
          {onOpenFile && content.change !== 'deleted' && (
            <>
              {' '}
              <button className="underline hover:text-[var(--color-text-primary)] cursor-pointer" onClick={() => onOpenFile(row.path)}>
                Open in editor
              </button>
            </>
          )}
        </Card>
      )
    case 'identical':
      return (
        <Card title="No text changes" testId="diff-identical">
          The content is the same; only the file mode or type changed.
        </Card>
      )
    case 'text':
      return (
        <MergeDiffEditor
          oldText={content.oldText}
          newText={content.newText}
          filePath={row.path}
          layout={props.layout}
          wrap={props.wrap}
          collapse={props.collapse}
          fill={props.fill}
          onChunks={props.onChunks}
          onCursor={props.onCursor}
          apiRef={props.apiRef}
          notes={editorNotes}
        />
      )
  }
}
