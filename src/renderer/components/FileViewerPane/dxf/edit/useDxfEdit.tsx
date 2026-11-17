// Editing, wired for the DXF viewer. Only the drawing-model path selects and
// edits (the viewer's own reading of the text has no handles). It gives the
// viewer:
//   - `tool`: the sheet's left-button gestures (select, box, drag to move,
//     the handles to turn and scale, a click to place a block);
//   - `marks` (in the SVG): the selection with its handles, agents' holds
//     and the ghost's layer;
//   - `over` (over the sheet): the selection box, the hold question, a hint;
//   - `toolbar` (in the header row): "Insert block";
//   - `panel` (beside the sheet): the selection, placing a block, or with
//     nothing selected "Insert block…"; `viewOnly` (under the header: why not);
//   - `onKey`: Esc stops a drag or a block being placed, else clears the selection.

import { useEffect, useMemo, useRef } from 'react'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import type { PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { CadChange } from '../cad-change'
import type { ModelHeader, ModelRow } from '../model/types'
import { daemonEditSource, type EditSource } from './edit-source'
import { domBox, domGhost, EditMarks, HoldAsk, SelectBoxEl } from './EditLayer'
import { writesUtf8 } from './attrib-value'
import { insertableBlocks } from './blocks'
import { heldHandles } from './holds'
import { InsertButton, InsertPanel, NothingSelectedPanel, useInsertFlow } from './InsertPanel'
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
  toolbar: React.ReactNode
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

  const blocks = useMemo(() => (p.drawing ? insertableBlocks(p.drawing.header) : []), [p.drawing])
  const flow = useInsertFlow(edit, p.drawing?.rows ?? [])

  const ghostRoot = useRef<SVGGElement>(null)
  const boxEl = useRef<HTMLDivElement>(null)
  const state = useRef({ edit, flow, padUnits: p.padUnits, reduced })
  state.current = { edit, flow, padUnits: p.padUnits, reduced }

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

  // A block being placed: its ghost waits for the pointer; it stays where it was placed while the form asks for labels, and goes when placing stops.
  const placing = flow.step?.step === 'place' ? flow.step.block : null
  const inserting = !!flow.step
  useEffect(() => {
    if (placing) ghostApi.block(placing.handle, placing.base, placing.bbox)
    else if (!inserting) ghostApi.clear()
  }, [placing, inserting, ghostApi])
  // Selecting something stops placing a block.
  const cancelFlow = flow.cancel
  useEffect(() => {
    if (edit.selection.length) cancelFlow()
    // Only when the selection changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [edit.selection])

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
        placing: () => {
          const s = state.current.flow.step
          return s?.step === 'place' ? s.block.base : null
        },
        place: (at) => state.current.flow.placed(at),
        ghost: ghostApi,
        box,
      }
    })
  }, [p.drawing, ghostApi, box])

  const held = useMemo(() => (p.drawing ? heldHandles(edit.holds, edit.rows, p.drawing.header) : new Map<string, string>()), [edit.holds, edit.rows, p.drawing])

  const whoIs = async (pid: string) => (source && p.filePath ? source.whoIs(p.filePath, pid) : { name: null, self: false })
  if (!p.drawing) return { tool: null, marks: null, over: null, toolbar: null, panel: null, viewOnly: null, onKey: () => {}, goViewOnly: edit.goViewOnly, whoIs }

  const onKey = (e: React.KeyboardEvent): void => {
    if (e.key !== 'Escape' || e.defaultPrevented) return
    if (edit.prompt) return edit.prompt.no()
    if (tool?.cancel()) return
    if (flow.step) return flow.cancel()
    if (edit.selection.length) edit.select([])
  }
  const canInsert = edit.canEdit && blocks.length > 0
  const insertOff = !edit.editable || edit.busy

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
        {placing ? (
          <div className="pointer-events-none absolute bottom-2.5 left-3 right-3 text-[11px] text-[var(--dxf-note)]" data-testid="dxf-edit-hint">
            Click to place {placing.name}. Press Esc to stop. Hold Space and drag to pan.
          </div>
        ) : !edit.selection.length && !inserting ? (
          <div className="pointer-events-none absolute bottom-2.5 left-3 right-3 text-[11px] text-[var(--dxf-note)]" data-testid="dxf-edit-hint">
            Click to select. Drag left to right to select what’s inside, right to left for what it touches. Hold Space and drag to pan.
          </div>
        ) : null}
      </>
    ),
    toolbar: canInsert ? <InsertButton onClick={flow.start} disabled={insertOff || inserting} /> : null,
    panel: inserting ? (
      <InsertPanel flow={flow} blocks={blocks} edit={edit} utf8={writesUtf8(p.drawing.header.acadver)} />
    ) : edit.selection.length ? (
      <SelectionPanel edit={edit} header={p.drawing.header} slots={p.live.slots} />
    ) : canInsert ? (
      <NothingSelectedPanel onInsert={flow.start} disabled={insertOff} />
    ) : null,
    viewOnly: edit.viewOnly ? (
      <div className="shrink-0 px-3 pb-1 text-[11px] text-[var(--color-text-secondary)]" data-testid="dxf-view-only">
        {edit.viewOnly}
      </div>
    ) : null,
  }
}
