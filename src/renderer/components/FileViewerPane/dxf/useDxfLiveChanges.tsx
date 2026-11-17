// The live change highlight, wired for the DXF viewer: the source (the
// daemon's, unless one is passed in), the run's change, its outlines and its
// note. The viewer puts `outlines` inside its SVG and `note` under its header.

import { useMemo } from 'react'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import type { Bounds, DxfDrawing } from '../dxfModel'
import type { PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { CadChange } from './cad-change'
import { changeAuthors, changeMarks, marksBox, SELF_PID } from './cad-change-marks'
import { daemonCadChangeSource, type CadChangeSource } from './cad-change-source'
import { changeNote } from './cad-change-words'
import { ChangeNote, ChangeOutlines } from './CadChangeLayer'
import { useCadChanges } from './useCadChanges'

export function useDxfLiveChanges(p: {
  drawing: DxfDrawing | null
  filePath: string | undefined
  /** A source to use instead of the daemon's (tests, shots); null turns the highlight off. */
  changes: CadChangeSource | null | undefined
  frame: (b: Bounds) => void
  /** A few screen pixels in drawing units, to pad the outlines. */
  padUnits: () => number
}): {
  outlines: React.ReactNode
  note: React.ReactNode
  showOwn: (c: CadChange) => void
  slots: ReadonlyMap<string, PresenceSlot>
  meet: (pids: readonly string[]) => void
} {
  const scope = useOptionalRoom()?.scope ?? null
  const daemon = useMemo(() => (scope ? daemonCadChangeSource(scope) : null), [scope])
  const source = p.changes === undefined ? daemon : p.changes
  const live = useCadChanges(p.filePath ? source : null, scope, p.filePath ?? null)
  const { drawing } = p
  const marks = useMemo(() => (live.change && drawing ? changeMarks(live.change, live.slots, drawing) : null), [live.change, live.slots, drawing])
  const note = useMemo(() => (live.change && drawing ? changeNote(live.change, drawing) : null), [live.change, drawing])
  const { showOwn, slots, meet } = live
  if (!live.change || !marks || !note) return { outlines: null, note: null, showOwn, slots, meet }

  const box = marksBox(marks)
  const dots = [...new Set(changeAuthors(live.change).map((a) => (a === SELF_PID ? 'self' : live.slots.get(a))).filter((s) => s !== undefined))]
  return {
    showOwn,
    slots,
    meet,
    outlines: live.phase && (marks.marks.length || marks.links.length) ? <ChangeOutlines m={marks} phase={live.phase} pad={p.padUnits()} /> : null,
    note: (
      <ChangeNote
        text={note.text}
        slots={live.change.pairing === 'kept' ? dots : []}
        undoState={live.undoState}
        canUndo={note.undo !== null}
        canShow={box !== null}
        onShow={() => box && p.frame({ minX: box[0], minY: box[1], maxX: box[2], maxY: box[3] })}
        onUndo={() => note.undo && live.undo(note.undo)}
        onDismiss={live.dismiss}
      />
    ),
  }
}
