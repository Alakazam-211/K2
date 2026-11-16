// Where a file tab opens while its editor's preview switch is on ("New
// markdown editor" or "New code editor", lib/editor/preview-switch.ts): the
// new editor (EditorTab), or today's view. Live, any-file and deck files keep
// today's view until E4 (the new editors have no live editing yet, and the
// daemon writes a live file), and a locked (not text) file keeps its lock. For
// a live file, today's view says so in one line (`kept-live`), so the switch
// never looks broken.

import type { FileCategory } from '@/components/FileViewerPane/fileCategory'
import { newEditorKind, useEditorPreview, type EditorKind } from '@/lib/editor/preview-switch'

/** The new editor for this file while its switch is on, else null. Follows the switch live. */
export function useEditorFrameKind(category: FileCategory): EditorKind | null {
  const kind = newEditorKind(category)
  const on = useEditorPreview(kind ?? 'code')
  return kind && on ? kind : null
}

export type EditorFrameRoute = 'frame' | 'today' | 'kept-live'

/** `kind` is {@link useEditorFrameKind}'s answer; the rest is the file tab's state. */
export function editorFrameRoute(
  kind: EditorKind | null,
  s: { locked: boolean; liveOn: boolean; anyFile: boolean; deck: boolean },
): EditorFrameRoute {
  if (!kind || s.locked) return 'today'
  return s.liveOn || s.anyFile || s.deck ? 'kept-live' : 'frame'
}

/** Plain words, and no way to turn live editing off from here: that would end multiplayer for the file. */
export const editorPreviewLiveNote = (kind: EditorKind): string => `The new ${kind} editor doesn't open live files yet. It will soon.`

/** The line today's view shows, under its toolbar, for `kept-live`. */
export function EditorPreviewLiveNote({ kind }: { kind: EditorKind | null }): React.JSX.Element | null {
  if (!kind) return null
  return (
    <div role="status" data-testid="editor-preview-live-note" className="type-row border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1.5 text-[var(--color-text-primary)] flex-shrink-0">
      {editorPreviewLiveNote(kind)}
    </div>
  )
}
