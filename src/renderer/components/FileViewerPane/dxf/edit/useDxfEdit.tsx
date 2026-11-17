// Editing, wired for the DXF viewer. Only the drawing-model path selects and
// edits (the viewer's own reading of the text has no handles). It gives the
// viewer:
//   - `tool`: the sheet's left-button gestures (select, box, drag to move,
//     the handles to turn and scale);
//   - `marks` (in the SVG): the selection with its handles, agents' holds
//     and the ghost's layer;
//   - `over` (over the sheet): the selection box, the hold question, a hint;
//   - `panel` (beside the sheet) and `viewOnly` (under the header: why not);
//   - `onKey`: Esc stops a drag, else clears the selection.

import { useEffect, useMemo, useRef } from 'react'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import type { PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { CadChange } from '../cad-change'
import type { ModelHeader, ModelRow } from '../model/types'
import { daemonEditSource, type EditSource } from './edit-source'
import { domBox, domGhost, EditMarks, HoldAsk, SelectBoxEl } from './EditLayer'
import { heldHandles } from './holds'
import { SelectionPanel } from './SelectionPanel'
import { editTool, type SheetTool } from './sheet-tool'
import { useCadEdit, type EditDrawing } from './useCadEdit'

export function useDxfEdit(p: {
  /** The drawing, when it came from the model (with its header and rows); null on the viewer's own reading. */
  drawing: (EditDrawing & { header: ModelHeader; rows: ModelRow[] }) | null
  filePath: string | undefined
  /** A source to use instead of the daemon's (tests, shots); null turns editing off. */
  edits: EditSource | null | undefined
  live: { showOwn: (c: CadChange, digest?: string | null) => void; meet: (pids: readonly string[]) => void; slots: ReadonlyMap<string, PresenceSlot> }
  reload: () => void
  /** A few screen pixels in drawing units, to pad the outlines. */
  padUnits: () => number
}): {
  tool: SheetTool | null
  marks: React.ReactNode
  over: React.ReactNode
  panel: React.ReactNode
  viewOnly: React.ReactNode
  onKey: (e: React.KeyboardEvent) => void
  goViewOnly: (words: string) => void
  /** A version's author as a name, from who's on the file. */
  whoIs: (pid: string) => Promise<{ name: string | null; self: boolean }>
} {
  const scope = useOptionalRoom()?.scope ?? null
  const daemon = useMemo(() => (scope ? daemonEditSource(scope) : null), [scope])
  const source = p.edits === undefined ? daemon : p.edits
  const reduced = useReducedMotion()
  const edit = useCadEdit({ drawing: p.drawing, filePath: p.filePath, source, showOwn: p.live.showOwn, reload: p.reload, meet: p.live.meet })

  const ghostRoot = useRef<SVGGElement>(null)
  const boxEl = useRef<HTMLDivElement>(null)
  const state = useRef({ edit, padUnits: p.padUnits, reduced })
  state.current = { edit, padUnits: p.padUnits, reduced }

  const ghostApi = useMemo(
    () =>
      domGhost(
        () => ghostRoot.current,
        () => state.current.edit.byHandle,
        () => state.current.padUnits(),
        () => state.current.reduced,
      ),
    [],
  )
  const box = useMemo(() => ({ show: (...a: Parameters<ReturnType<typeof domBox>['show']>) => domBox(boxEl.current).show(...a), clear: () => domBox(boxEl.current).clear() }), [])

  // A landed move's ghost stays until the drawing is read again.
  useEffect(() => {
    ghostApi.clear()
  }, [p.drawing, ghostApi])

  const tool = useMemo<SheetTool | null>(() => {
    if (!p.drawing) return null
    return editTool(() => {
      const e = state.current.edit
      return {
        rows: e.rows,
        selection: e.selection,
        select: e.select,
        canMove: (h) => e.editable && e.canMove(h),
        move: e.move,
        frame: () => (e.editable && e.canMove(e.selection) ? e.frame : null),
        rotate: e.rotate,
        scale: e.scale,
        ghost: ghostApi,
        box,
      }
    })
  }, [p.drawing, ghostApi, box])

  const held = useMemo(() => (p.drawing ? heldHandles(edit.holds, edit.rows, p.drawing.header) : new Map<string, string>()), [edit.holds, edit.rows, p.drawing])

  const whoIs = async (pid: string) => (source && p.filePath ? source.whoIs(p.filePath, pid) : { name: null, self: false })
  if (!p.drawing) return { tool: null, marks: null, over: null, panel: null, viewOnly: null, onKey: () => {}, goViewOnly: edit.goViewOnly, whoIs }

  const onKey = (e: React.KeyboardEvent): void => {
    if (e.key !== 'Escape' || e.defaultPrevented) return
    if (edit.prompt) return edit.prompt.no()
    if (tool?.cancel()) return
    if (edit.selection.length) edit.select([])
  }

  return {
    tool,
    onKey,
    goViewOnly: edit.goViewOnly,
    whoIs,
    marks: (
      <>
        <EditMarks
          selection={edit.selection}
          byHandle={edit.byHandle}
          held={held}
          slots={p.live.slots}
          pad={p.padUnits()}
          frame={edit.editable && !edit.busy && edit.canMove(edit.selection) ? edit.frame : null}
        />
        <g ref={ghostRoot} transform="scale(1 -1)" pointerEvents="none" data-testid="dxf-ghost-layer" />
      </>
    ),
    over: (
      <>
        <SelectBoxEl boxRef={boxEl} />
        {edit.prompt ? <HoldAsk prompt={edit.prompt} /> : null}
        {!edit.selection.length ? (
          <div className="pointer-events-none absolute bottom-2.5 left-3 right-3 text-[11px] text-[var(--dxf-note)]" data-testid="dxf-edit-hint">
            Click to select. Drag left to right to select what’s inside, right to left for what it touches. Hold Space and drag to pan.
          </div>
        ) : null}
      </>
    ),
    panel: edit.selection.length ? <SelectionPanel edit={edit} header={p.drawing.header} slots={p.live.slots} /> : null,
    viewOnly: edit.viewOnly ? (
      <div className="shrink-0 px-3 pb-1 text-[11px] text-[var(--color-text-secondary)]" data-testid="dxf-view-only">
        {edit.viewOnly}
      </div>
    ) : null,
  }
}
