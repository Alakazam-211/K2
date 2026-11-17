// The selection panel, beside the drawing (the drawing stays the hero):
//   - how many are selected, of what kinds, on which layers; one entity's
//     kind, layer and handle;
//   - agents working on them, by name and what for;
//   - what can't be changed here and why, in plain words (and then no Move,
//     Rotate or Scale);
//   - "Move by" in drawing units, for exact moves; "Rotate by" in degrees
//     and "Scale by" a factor, both about the selection's centre;
//   - a block reference's attributes as rows you can change (Enter saves),
//     checked as you type; a single-line text's value the same way; a
//     multi-line text's shown, not editable here yet;
//   - the last refusal, in plain words.
// With the drawing view only, it lists what's selected, with one quiet
// "View only" line (the header line says why); red is for refusals only.

import { useEffect, useId, useState } from 'react'
import { Button, Input } from '@/components/ui'
import { CloseGlyph } from '@/components/LiveView/shell/glyphs'
import { PRESENCE_TOKEN, type PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { ModelHeader, ModelRow } from '../model/types'
import { attribProblem, writesUtf8 } from './attrib-value'
import { whyWords } from './edit-words'
import { holdLine, holdsOn } from './holds'
import { capital, kindName, kindsLine } from './kinds'
import { editReason } from './selection'
import { RotateBy, ScaleBy } from './TurnBy'
import type { CadEdit } from './useCadEdit'

type Attrib = { tag: string; value: string; constant?: boolean }

const attribsOf = (r: ModelRow): Attrib[] =>
  (Array.isArray(r.geom?.attribs) ? (r.geom.attribs as unknown[]) : []).filter(
    (a): a is Attrib => !!a && typeof (a as Attrib).tag === 'string' && typeof (a as Attrib).value === 'string',
  )

/** 1-based place of attribute `i` among those with its tag, or null when the tag is there once. */
function nthOf(list: readonly Attrib[], i: number): number | null {
  const tag = list[i].tag.toUpperCase()
  const same = list.filter((a) => a.tag.toUpperCase() === tag)
  return same.length > 1 ? list.slice(0, i + 1).filter((a) => a.tag.toUpperCase() === tag).length : null
}

const textOf = (r: ModelRow): string => {
  const g = r.geom ?? {}
  if (typeof g.plain === 'string') return g.plain
  if (Array.isArray(g.paragraphs)) return (g.paragraphs as unknown[]).map((p) => (typeof (p as { plain?: unknown })?.plain === 'string' ? (p as { plain: string }).plain : '')).join('\n')
  return ''
}

export const LABEL = 'type-hint text-[var(--color-text-secondary)]'
/** The side panel beside the drawing. */
export const PANEL = 'flex w-[248px] shrink-0 flex-col overflow-y-auto border-l border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-2'

const ordinal = (n: number): string => `${n}${n % 100 >= 11 && n % 100 <= 13 ? 'th' : (['th', 'st', 'nd', 'rd'][n % 10] ?? 'th')}`

function Row({ k, v, testId }: { k: string; v: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <div className="flex gap-2 py-0.5">
      <dt className={`${LABEL} w-14 shrink-0`}>{k}</dt>
      <dd className="type-hint m-0 min-w-0 break-words text-[var(--color-text-primary)]" data-testid={testId}>
        {v}
      </dd>
    </div>
  )
}

function AttribRow({ a, nth, insert, edit, utf8 }: { a: Attrib; nth: number | null; insert: string; edit: CadEdit; utf8: boolean }): React.JSX.Element {
  return (
    <ValueField
      label={nth ? `${a.tag} (${ordinal(nth)})` : a.tag}
      value={a.value}
      constant={a.constant}
      edit={edit}
      utf8={utf8}
      save={(v) => void edit.relabel(insert, a.tag, v, nth)}
      testId="dxf-attrib"
      tag={a.tag}
    />
  )
}

/** One value you can change in the panel, checked as you type: Enter saves, Esc undoes the typing. */
function ValueField(p: {
  label: string
  value: string
  constant?: boolean
  text?: boolean
  edit: CadEdit
  utf8: boolean
  save: (value: string) => void
  testId: string
  tag?: string
}): React.JSX.Element {
  const { value, edit } = p
  const [draft, setDraft] = useState(value)
  const id = useId()
  useEffect(() => setDraft(value), [value])
  const problem = attribProblem(draft, { utf8: p.utf8, text: p.text })
  const changed = draft !== value
  const save = () => {
    if (!changed || problem || edit.busy) return
    p.save(draft)
  }
  return (
    <div className="py-1" data-testid={`${p.testId}-row`} data-tag={p.tag}>
      <label htmlFor={id} className={`${LABEL} block pb-0.5`}>
        {p.label}
      </label>
      {p.constant ? (
        <p className="type-hint m-0 text-[var(--color-text-primary)]">
          {value} <span className="text-[var(--color-text-secondary)]">· set by the block</span>
        </p>
      ) : (
        <>
          <Input
            id={id}
            value={draft}
            spellCheck={false}
            aria-invalid={problem ? true : undefined}
            aria-describedby={problem ? `${id}-why` : undefined}
            disabled={!edit.editable || edit.busy}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') save()
              // Esc in the field never clears the selection: it undoes the typing, or leaves the field.
              if (e.key === 'Escape') {
                e.stopPropagation()
                if (changed) setDraft(value)
                else e.currentTarget.blur()
              }
            }}
            className="w-full"
            data-testid={`${p.testId}-input`}
          />
          {problem ? (
            <p id={`${id}-why`} className="type-hint m-0 pt-0.5 text-[var(--color-status-error-text)]" data-testid={`${p.testId}-problem`}>
              {problem}
            </p>
          ) : changed ? (
            <p className={`${LABEL} m-0 pt-0.5`}>Press Enter to save, Esc to cancel.</p>
          ) : null}
        </>
      )}
    </div>
  )
}

function MoveBy({ edit }: { edit: CadEdit }): React.JSX.Element {
  const [dx, setDx] = useState('')
  const [dy, setDy] = useState('')
  const x = dx.trim() === '' ? 0 : Number(dx)
  const y = dy.trim() === '' ? 0 : Number(dy)
  const ok = Number.isFinite(x) && Number.isFinite(y) && (x !== 0 || y !== 0)
  const unit = edit.unit === 'drawing' ? 'drawing units' : edit.unit
  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!ok || !edit.editable || edit.busy) return
    void edit.move(x, y).then((landed) => {
      if (landed) {
        setDx('')
        setDy('')
      }
    })
  }
  const field = (label: string, v: string, set: (v: string) => void, testId: string) => (
    <label className="flex min-w-0 flex-1 items-center gap-1.5">
      <span className={LABEL}>{label}</span>
      <Input inputMode="decimal" value={v} placeholder="0" onChange={(e) => set(e.target.value)} className="w-full min-w-0" aria-label={`Move by ${label}, in ${unit}`} data-testid={testId} />
    </label>
  )
  return (
    <form onSubmit={submit} className="pt-1" data-testid="dxf-move-by">
      <p className={`${LABEL} m-0 pb-1`}>Move by, in {unit}</p>
      <div className="flex items-center gap-2">
        {field('X', dx, setDx, 'dxf-move-dx')}
        {field('Y', dy, setDy, 'dxf-move-dy')}
        <Button type="submit" size="sm" variant="default" disabled={!ok || !edit.editable || edit.busy} data-testid="dxf-move-go">
          Move
        </Button>
      </div>
    </form>
  )
}

export function SelectionPanel({ edit, header, slots }: { edit: CadEdit; header: ModelHeader; slots: ReadonlyMap<string, PresenceSlot> }): React.JSX.Element {
  const rows = edit.selection.map((h) => edit.byHandle.get(h)).filter((r): r is ModelRow => !!r)
  const one = rows.length === 1 ? rows[0] : null
  const layers = [...new Set(rows.map((r) => r.layer))]
  const reasons = [...new Set(rows.map((r) => editReason(r, header)).filter((w): w is string => w !== null))]
  // A read-only drawing's rows say so once, in the header line, not again here.
  const said = edit.viewOnly ? reasons.filter((w) => w !== 'read-only') : reasons
  // A locked layer is named: the layer of a selected row that's on one.
  const lockedOn = rows.find((r) => editReason(r, header) === 'locked-layer')?.layer ?? null
  const cantWords = [...new Set(said.map((w) => whyWords(w, w === 'locked-layer' ? lockedOn : null)))]
  const holds = holdsOn(edit.selection, edit.holds, rows, header)
  const controls = edit.canEdit
  const isText = one && (one.kind === 'TEXT' || one.kind === 'MTEXT')
  const textEditable = one?.kind === 'TEXT' && controls && !reasons.length
  const attribs = one?.kind === 'INSERT' ? attribsOf(one) : []
  const utf8 = writesUtf8(header.acadver)
  return (
    <aside
      aria-label="Selection"
      data-testid="dxf-edit-panel"
      className={PANEL}
    >
      <div className="flex items-center gap-2">
        <h3 className="type-row m-0 flex-1 font-semibold text-[var(--color-text-primary)]" data-testid="dxf-edit-count">
          {rows.length} selected
        </h3>
        <button
          type="button"
          data-k2-focus=""
          aria-label="Clear the selection (Esc)"
          title="Clear the selection (Esc)"
          onClick={() => edit.select([])}
          data-testid="dxf-edit-clear"
          className="inline-flex h-[22px] w-[22px] flex-none cursor-pointer items-center justify-center rounded-[var(--radius-field)] text-[var(--color-text-secondary)] hover:bg-[var(--color-control-hover-quiet)] hover:text-[var(--color-text-primary)]"
        >
          <CloseGlyph size={12} />
        </button>
      </div>
      {one ? (
        <dl className="m-0 pt-1" data-testid="dxf-edit-one">
          <Row k="Kind" v={capital(kindName(one.kind))} testId="dxf-edit-kind" />
          {one.kind === 'INSERT' && typeof one.geom?.name === 'string' ? <Row k="Name" v={one.geom.name} testId="dxf-edit-name" /> : null}
          <Row k="Layer" v={one.layer} />
          <Row k="Handle" v={one.h.replace(/^@h:/, '')} />
        </dl>
      ) : (
        <p className="type-hint m-0 pt-1 text-[var(--color-text-primary)]" data-testid="dxf-edit-kinds">
          {kindsLine(rows.map((r) => r.kind))}
          <span className="block text-[var(--color-text-secondary)]">{layers.length === 1 ? `On ${layers[0]}` : `On ${layers.length} layers`}</span>
        </p>
      )}
      {edit.viewOnly ? (
        <p className={`${LABEL} m-0 pt-1`} data-testid="dxf-edit-view-only">
          View only
        </p>
      ) : null}
      {holds.length ? (
        <p className="type-hint m-0 flex gap-1.5 pt-2 text-[var(--color-text-primary)]" data-testid="dxf-edit-held">
          <svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true" className="mt-[5px] flex-none">
            <circle cx="4" cy="4" r="4" fill={slots.get(holds[0].pid) ? PRESENCE_TOKEN[slots.get(holds[0].pid)!] : 'var(--color-text-secondary)'} />
          </svg>
          <span>{holdLine(holds, rows.length)}</span>
        </p>
      ) : null}
      {edit.message ? (
        <p role="alert" className="type-hint m-0 pt-2 text-[var(--color-status-error-text)]" data-testid="dxf-edit-message">
          {edit.message}
        </p>
      ) : null}
      {cantWords.length ? (
        <div className="pt-2" data-testid="dxf-edit-cant">
          <p className={`${LABEL} m-0`}>{rows.length === 1 ? 'Can’t change this here' : 'Can’t change these here'}</p>
          {cantWords.map((w) => (
            <p key={w} className="type-hint m-0 pt-0.5 text-[var(--color-text-primary)]">
              {w}
            </p>
          ))}
        </div>
      ) : null}
      {controls && !reasons.length ? (
        <>
          <MoveBy edit={edit} />
          <RotateBy edit={edit} />
          <ScaleBy edit={edit} />
        </>
      ) : null}
      {attribs.length && one ? (
        <div className="pt-2" data-testid="dxf-attribs">
          <p className={`${LABEL} m-0`}>Attributes</p>
          {attribs.map((a, i) =>
            controls && !reasons.length ? (
              <AttribRow key={`${a.tag}-${i}`} a={a} nth={nthOf(attribs, i)} insert={one.h} edit={edit} utf8={utf8} />
            ) : (
              <Row key={`${a.tag}-${i}`} k={a.tag} v={a.value} />
            ),
          )}
        </div>
      ) : null}
      {isText && textEditable ? (
        <div className="pt-2" data-testid="dxf-edit-text">
          <ValueField label="Text" value={textOf(one)} text edit={edit} utf8={utf8} save={(v) => void edit.setText(one.h, v)} testId="dxf-text" />
        </div>
      ) : isText ? (
        <div className="pt-2" data-testid="dxf-edit-text">
          <p className={`${LABEL} m-0`}>Text</p>
          <p className="type-hint m-0 whitespace-pre-wrap break-words text-[var(--color-text-primary)]">{textOf(one)}</p>
          {controls && one.kind === 'MTEXT' ? <p className={`${LABEL} m-0 pt-0.5`}>Multi-line text: later.</p> : null}
        </div>
      ) : null}
      {edit.busy ? (
        <p className={`${LABEL} m-0 pt-2`} role="status">
          Saving…
        </p>
      ) : null}
    </aside>
  )
}
