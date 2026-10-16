// Where the review notes go on the text the viewer shows (C5). Pure: no
// React, no CodeMirror.

import { locateNote, splitLines, type NoteSide, type ReviewNote } from '@/stores/git-notes'
import type { NoteMark } from './notesExtension'

/** Open notes on `path`'s `side`, placed on `text`: on their lines, or
 *  where those lines moved; an outdated one sits at its old line (clamped
 *  to the text) with `outdated: true`. */
export function placeNotes(notes: readonly ReviewNote[], path: string, side: NoteSide, text: string): NoteMark[] {
  const total = Math.max(splitLines(text).length, 1)
  const out: NoteMark[] = []
  for (const n of notes) {
    if (n.resolvedAt != null || n.path !== path || n.side !== side) continue
    const at = locateNote(n, text)
    const line = at ? at.start : Math.min(n.startLine, total)
    const endLine = at ? at.end : Math.max(line, Math.min(n.endLine, total))
    out.push({
      id: n.id,
      line,
      endLine,
      body: n.body,
      authorName: n.authorName,
      sent: n.sentAt != null,
      outdated: !at,
    })
  }
  return out
}
