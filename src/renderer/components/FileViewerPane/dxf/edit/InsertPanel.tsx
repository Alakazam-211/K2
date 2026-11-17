// Placing a block, in three steps, in the side panel beside the drawing:
//   1. pick one of the drawing's blocks that can be placed;
//   2. its ghost follows the pointer over the drawing, and a click places it
//      (its base point goes where you click);
//   3. a short form says where it goes (the drawing's current layer) and
//      asks for its labels (the attributes that aren't constant, each by its
//      prompt), with their defaults filled in, checked as you type; Insert
//      places it. A block with no labels is placed on the click.
// The new block is selected once the drawing shows it; a refusal says why
// and keeps the form. Esc or Cancel stops at any step.
// With nothing selected, the panel offers "Insert block…"; the toolbar has
// "Insert block" too.

import { useCallback, useId, useState } from 'react'
import { Button, Input } from '@/components/ui'
import type { ModelBlock, ModelRow, XY } from '../model/types'
import { attribProblem } from './attrib-value'
import { insertLabels, type InsertLabel } from './blocks'
import { fmtCoord } from './edit-flow'
import { LABEL, PANEL } from './SelectionPanel'
import type { CadEdit } from './useCadEdit'

export type InsertStep = { step: 'pick' } | { step: 'place'; block: ModelBlock } | { step: 'form'; block: ModelBlock; at: XY; labels: InsertLabel[] }

export type InsertFlow = {
  step: InsertStep | null
  start: () => void
  choose: (block: ModelBlock) => void
  /** The click: a block with labels asks for them, one without is placed. */
  placed: (at: XY) => void
  commit: (values: readonly string[]) => Promise<boolean>
  cancel: () => void
}

export function useInsertFlow(edit: CadEdit, rows: readonly ModelRow[]): InsertFlow {
  const [step, setStep] = useState<InsertStep | null>(null)
  const send = useCallback(
    async (block: ModelBlock, at: XY, labels: readonly InsertLabel[], values: readonly string[]) => {
      // A label whose default isn't known is sent only when filled in (else the block's own default stays).
      const pairs = labels.flatMap((l, i) => (l.known || values[i] !== '' ? [[l.tag, values[i]] as const] : []))
      const landed = await edit.insert(block.handle, at, pairs)
      if (landed) setStep(null)
      return landed
    },
    [edit],
  )
  return {
    step,
    start: () => {
      edit.select([])
      setStep({ step: 'pick' })
    },
    choose: (block) => setStep({ step: 'place', block }),
    placed: (at) => {
      if (step?.step !== 'place') return
      const labels = insertLabels(step.block, rows)
      setStep({ step: 'form', block: step.block, at, labels })
      if (!labels.length) void send(step.block, at, labels, [])
    },
    commit: (values) => (step?.step === 'form' ? send(step.block, step.at, step.labels, values) : Promise.resolve(false)),
    cancel: () => setStep(null),
  }
}

/** The toolbar's action. */
export function InsertButton({ onClick, disabled }: { onClick: () => void; disabled: boolean }): React.JSX.Element {
  return (
    <button
      type="button"
      className="px-1.5 py-0.5 text-[10px] font-medium border border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-50"
      onClick={onClick}
      disabled={disabled}
      title="Place one of this drawing’s blocks"
      data-testid="dxf-insert-block"
    >
      Insert block
    </button>
  )
}

/** With nothing selected: what to do next. */
export function NothingSelectedPanel({ onInsert, disabled }: { onInsert: () => void; disabled: boolean }): React.JSX.Element {
  return (
    <aside aria-label="Selection" data-testid="dxf-edit-empty" className={PANEL}>
      <h3 className="type-row m-0 font-semibold text-[var(--color-text-primary)]">Nothing selected</h3>
      <p className={`${LABEL} m-0 pt-1`}>Click a shape to select it.</p>
      <div className="pt-2">
        <Button size="sm" variant="default" onClick={onInsert} disabled={disabled} data-testid="dxf-insert-open">
          Insert block…
        </Button>
      </div>
    </aside>
  )
}

export function InsertPanel(p: { flow: InsertFlow; blocks: readonly ModelBlock[]; edit: CadEdit; utf8: boolean; layer: string | null }): React.JSX.Element | null {
  const { flow, blocks, edit, utf8 } = p
  const s = flow.step
  if (!s) return null
  return (
    <aside aria-label="Insert a block" data-testid="dxf-insert-panel" className={PANEL}>
      <h3 className="type-row m-0 font-semibold text-[var(--color-text-primary)]">{s.step === 'pick' ? 'Insert a block' : `Insert ${s.block.name}`}</h3>
      {s.step === 'pick' ? (
        blocks.length ? (
          <ul className="m-0 list-none p-0 pt-1" data-testid="dxf-insert-picker">
            {blocks.map((b) => (
              <li key={b.handle}>
                <button
                  type="button"
                  data-k2-focus=""
                  onClick={() => flow.choose(b)}
                  data-testid="dxf-insert-choice"
                  data-block={b.handle}
                  className="type-row flex w-full cursor-pointer items-center rounded-[var(--radius-field)] px-1.5 py-1 text-left text-[var(--color-text-primary)] hover:bg-[var(--color-control-hover-quiet)]"
                >
                  {b.name}
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className={`${LABEL} m-0 pt-1`}>This drawing has no blocks to place.</p>
        )
      ) : s.step === 'place' ? (
        <p className="type-hint m-0 pt-1 text-[var(--color-text-primary)]" data-testid="dxf-insert-placing">
          Click in the drawing to place it. Its base point goes where you click.
        </p>
      ) : (
        <InsertForm key={`${s.block.handle}-${s.at.join()}`} step={s} flow={flow} edit={edit} utf8={utf8} layer={p.layer} />
      )}
      {edit.message ? (
        <p role="alert" className="type-hint m-0 pt-2 text-[var(--color-status-error-text)]" data-testid="dxf-edit-message">
          {edit.message}
        </p>
      ) : null}
      {edit.busy ? (
        <p className={`${LABEL} m-0 pt-2`} role="status">
          Saving…
        </p>
      ) : null}
      {s.step !== 'form' ? (
        <div className="pt-2">
          <Button size="sm" variant="quiet" onClick={flow.cancel} data-testid="dxf-insert-cancel">
            Cancel
          </Button>
        </div>
      ) : null}
    </aside>
  )
}

function InsertForm({ step, flow, edit, utf8, layer }: { step: Extract<InsertStep, { step: 'form' }>; flow: InsertFlow; edit: CadEdit; utf8: boolean; layer: string | null }): React.JSX.Element {
  const [values, setValues] = useState(() => step.labels.map((l) => l.value))
  const id = useId()
  const problems = values.map((v) => attribProblem(v, { utf8 }))
  const ok = problems.every((p) => p === null) && edit.editable && !edit.busy
  const unit = edit.unit === 'drawing' ? '' : ` ${edit.unit}`
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        if (ok) void flow.commit(values)
      }}
      data-testid="dxf-insert-form"
    >
      <p className={`${LABEL} m-0 pt-1`} data-testid="dxf-insert-at">
        At {fmtCoord(step.at[0])}, {fmtCoord(step.at[1])}
        {unit}
      </p>
      {layer ? (
        <p className={`${LABEL} m-0`} data-testid="dxf-insert-layer">
          Goes on layer {layer}
        </p>
      ) : null}
      {step.labels.length ? <p className={`${LABEL} m-0 pt-2`}>Labels</p> : null}
      {step.labels.map((l, i) => (
        <div key={l.tag} className="py-1" data-testid="dxf-insert-label" data-tag={l.tag}>
          <label htmlFor={`${id}-${i}`} className={`${LABEL} block pb-0.5`}>
            {l.prompt || l.tag}
          </label>
          <Input
            id={`${id}-${i}`}
            value={values[i]}
            spellCheck={false}
            placeholder={l.known ? undefined : 'The block’s default'}
            aria-invalid={problems[i] ? true : undefined}
            aria-describedby={problems[i] ? `${id}-${i}-why` : undefined}
            onChange={(e) => setValues((v) => v.map((x, j) => (j === i ? e.target.value : x)))}
            className="w-full"
            data-testid="dxf-insert-input"
          />
          {problems[i] ? (
            <p id={`${id}-${i}-why`} className="type-hint m-0 pt-0.5 text-[var(--color-status-error-text)]">
              {problems[i]}
            </p>
          ) : null}
        </div>
      ))}
      <div className="flex justify-end gap-2 pt-2">
        <Button type="button" size="sm" variant="quiet" onClick={flow.cancel} data-testid="dxf-insert-cancel">
          Cancel
        </Button>
        <Button type="submit" size="sm" variant="accent" disabled={!ok} data-testid="dxf-insert-go">
          Insert
        </Button>
      </div>
    </form>
  )
}
