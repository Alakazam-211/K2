// Review notes inside a CodeMirror diff editor (C5): a gutter to start a
// note (click a line's gutter; with lines selected the note covers them, or
// press `c`), a marker on noted lines, the saved notes as cards under their
// last line, and an inline composer. Plain DOM widgets: the editor is
// read-only and the cards are small; the data lives in the daemon and comes
// in as `setNoteMarks`.

import { StateEffect, StateField, RangeSetBuilder, type EditorState, type Extension } from '@codemirror/state'
import { Decoration, EditorView, GutterMarker, WidgetType, gutter, keymap, type DecorationSet } from '@codemirror/view'

/** One open note placed on this editor's text. */
export interface NoteMark {
  id: string
  /** 1-based, where the note is shown now (followed when the lines moved). */
  line: number
  endLine: number
  body: string
  authorName: string
  sent: boolean
  /** Its lines are no longer in this text; shown at the old line. */
  outdated: boolean
}

export interface NotesHandlers {
  canWrite: boolean
  /** New note on lines `from..to` of this side; `anchor` = their text. */
  save(from: number, to: number, body: string, anchor: string): Promise<void>
  edit(id: string, body: string): Promise<void>
  resolve(id: string): Promise<void>
  remove(id: string): Promise<void>
}

interface Composer {
  from: number
  to: number
  editId?: string
  body?: string
}

interface NotesState {
  marks: NoteMark[]
  canWrite: boolean
  composer: Composer | null
}

export const setNoteMarks = StateEffect.define<{ marks: NoteMark[]; canWrite: boolean }>()
export const setNoteComposer = StateEffect.define<Composer | null>()

const notesField = StateField.define<NotesState>({
  create: () => ({ marks: [], canWrite: false, composer: null }),
  update(value, tr) {
    let next = value
    for (const e of tr.effects) {
      if (e.is(setNoteMarks)) next = { ...next, marks: e.value.marks, canWrite: e.value.canWrite }
      if (e.is(setNoteComposer)) next = { ...next, composer: e.value }
    }
    if (next.composer && next.composer.to > tr.state.doc.lines) next = { ...next, composer: null }
    return next
  },
})

function lineOf(state: EditorState, n: number): number {
  return Math.min(Math.max(n, 1), state.doc.lines)
}

/** The lines a new note covers when started on `line`: the selection's
 *  lines when it is not empty and touches `line`, else just `line`. */
export function noteRangeFor(state: EditorState, line: number): { from: number; to: number } {
  const sel = state.selection.main
  if (!sel.empty) {
    const from = state.doc.lineAt(sel.from).number
    let to = state.doc.lineAt(sel.to).number
    // A selection ending at the start of a line doesn't include it.
    if (to > from && state.doc.line(to).from === sel.to) to -= 1
    if (line >= from && line <= to) return { from, to }
  }
  return { from: line, to: line }
}

function button(label: string, title: string, onClick: () => void, testId: string): HTMLButtonElement {
  const b = document.createElement('button')
  b.type = 'button'
  b.textContent = label
  b.title = title
  b.className = 'cm-k2-note-btn'
  b.dataset.testid = testId
  b.addEventListener('mousedown', (e) => e.stopPropagation())
  b.addEventListener('click', (e) => {
    e.preventDefault()
    e.stopPropagation()
    onClick()
  })
  return b
}

class NoteCard extends WidgetType {
  constructor(
    readonly mark: NoteMark,
    readonly canWrite: boolean,
    readonly handlers: { current: NotesHandlers | null },
  ) {
    super()
  }
  eq(other: NoteCard): boolean {
    const a = this.mark
    const b = other.mark
    return (
      a.id === b.id &&
      a.body === b.body &&
      a.sent === b.sent &&
      a.outdated === b.outdated &&
      a.line === b.line &&
      a.endLine === b.endLine &&
      a.authorName === b.authorName &&
      this.canWrite === other.canWrite
    )
  }
  toDOM(view: EditorView): HTMLElement {
    const m = this.mark
    const card = document.createElement('div')
    card.className = 'cm-k2-note'
    card.dataset.noteId = m.id
    card.dataset.testid = 'note-card'
    if (m.outdated) card.dataset.outdated = ''
    const head = document.createElement('div')
    head.className = 'cm-k2-note-head'
    const who = document.createElement('span')
    const where = m.line === m.endLine ? `line ${m.line}` : `lines ${m.line}–${m.endLine}`
    who.textContent = `${m.authorName || 'someone'} · ${where}`
    head.appendChild(who)
    for (const [flag, label] of [
      [m.sent, 'sent'],
      [m.outdated, 'outdated'],
    ] as const) {
      if (!flag) continue
      const tag = document.createElement('span')
      tag.className = `cm-k2-note-tag cm-k2-note-tag-${label}`
      tag.textContent = label
      head.appendChild(tag)
    }
    if (this.canWrite) {
      const tools = document.createElement('span')
      tools.className = 'cm-k2-note-tools'
      tools.appendChild(
        button('Edit', 'Edit this note', () => {
          view.dispatch({ effects: setNoteComposer.of({ from: m.line, to: m.endLine, editId: m.id, body: m.body }) })
        }, 'note-edit'),
      )
      tools.appendChild(button('Resolve', 'Resolve this note', () => void this.handlers.current?.resolve(m.id), 'note-resolve'))
      tools.appendChild(button('Delete', 'Delete this note', () => void this.handlers.current?.remove(m.id), 'note-delete'))
      head.appendChild(tools)
    }
    const body = document.createElement('div')
    body.className = 'cm-k2-note-body'
    body.textContent = m.body
    card.append(head, body)
    return card
  }
  ignoreEvent(): boolean {
    return true
  }
}

class ComposerWidget extends WidgetType {
  constructor(
    readonly composer: Composer,
    readonly handlers: { current: NotesHandlers | null },
  ) {
    super()
  }
  eq(other: ComposerWidget): boolean {
    const a = this.composer
    const b = other.composer
    return a.from === b.from && a.to === b.to && a.editId === b.editId
  }
  toDOM(view: EditorView): HTMLElement {
    const c = this.composer
    const wrap = document.createElement('div')
    wrap.className = 'cm-k2-note cm-k2-note-composer'
    wrap.dataset.testid = 'note-composer'
    const label = document.createElement('div')
    label.className = 'cm-k2-note-head'
    label.textContent = c.editId
      ? 'Edit note'
      : c.from === c.to
        ? `Note on line ${c.from}`
        : `Note on lines ${c.from}–${c.to}`
    const ta = document.createElement('textarea')
    ta.className = 'cm-k2-note-input'
    ta.rows = 3
    ta.placeholder = 'What should the agent change? (⌘/Ctrl+Enter to save, Esc to cancel)'
    ta.value = c.body ?? ''
    ta.dataset.testid = 'note-input'
    const err = document.createElement('div')
    err.className = 'cm-k2-note-error'
    const close = (): void => view.dispatch({ effects: setNoteComposer.of(null) })
    let busy = false
    const save = async (): Promise<void> => {
      const text = ta.value.trim()
      const h = this.handlers.current
      if (!text || busy || !h) return
      busy = true
      saveBtn.disabled = true
      err.textContent = ''
      try {
        if (c.editId) {
          await h.edit(c.editId, text)
        } else {
          const from = lineOf(view.state, c.from)
          const to = lineOf(view.state, c.to)
          const anchor: string[] = []
          for (let n = from; n <= to; n++) anchor.push(view.state.doc.line(n).text)
          await h.save(from, to, text, anchor.join('\n'))
        }
        close()
      } catch (e) {
        err.textContent = e instanceof Error ? e.message : String(e)
        busy = false
        saveBtn.disabled = false
      }
    }
    ta.addEventListener('keydown', (e) => {
      e.stopPropagation()
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault()
        void save()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        close()
      }
    })
    const row = document.createElement('div')
    row.className = 'cm-k2-note-tools'
    const saveBtn = button(c.editId ? 'Save' : 'Add note', 'Save (⌘/Ctrl+Enter)', () => void save(), 'note-save')
    row.append(saveBtn, button('Cancel', 'Cancel (Esc)', close, 'note-cancel'))
    wrap.append(label, ta, err, row)
    setTimeout(() => ta.focus(), 0)
    return wrap
  }
  ignoreEvent(): boolean {
    return true
  }
}

class DotMarker extends GutterMarker {
  constructor(readonly count: number, readonly outdated: boolean) {
    super()
  }
  eq(other: DotMarker): boolean {
    return other.count === this.count && other.outdated === this.outdated
  }
  toDOM(): Node {
    const s = document.createElement('span')
    s.className = `cm-k2-note-dot${this.outdated ? ' cm-k2-note-dot-outdated' : ''}`
    s.textContent = this.count > 1 ? String(this.count) : '●'
    s.title = `${this.count} review note${this.count === 1 ? '' : 's'}`
    s.dataset.testid = 'note-dot'
    return s
  }
}

class AddMarker extends GutterMarker {
  toDOM(): Node {
    const s = document.createElement('span')
    s.className = 'cm-k2-note-add'
    s.textContent = '+'
    s.title = 'Add a review note (select lines first to cover them)'
    return s
  }
}
const ADD = new AddMarker()

function decorations(state: EditorState, handlers: { current: NotesHandlers | null }): DecorationSet {
  const { marks, canWrite, composer } = state.field(notesField)
  const items: Array<{ pos: number; deco: Decoration }> = []
  for (const m of marks) {
    if (composer?.editId === m.id) continue
    const pos = state.doc.line(lineOf(state, m.endLine)).to
    items.push({ pos, deco: Decoration.widget({ widget: new NoteCard(m, canWrite, handlers), block: true, side: 1 }) })
  }
  if (composer && canWrite) {
    const pos = state.doc.line(lineOf(state, composer.to)).to
    items.push({ pos, deco: Decoration.widget({ widget: new ComposerWidget(composer, handlers), block: true, side: 2 }) })
  }
  items.sort((a, b) => a.pos - b.pos)
  const b = new RangeSetBuilder<Decoration>()
  for (const it of items) b.add(it.pos, it.pos, it.deco)
  return b.finish()
}

/** Open the composer on `line` (the selection's lines when it covers it). */
export function startNoteAt(view: EditorView, line: number): boolean {
  const { canWrite } = view.state.field(notesField)
  if (!canWrite) return false
  const n = lineOf(view.state, line)
  view.dispatch({ effects: setNoteComposer.of(noteRangeFor(view.state, n)) })
  return true
}

/** The extension for one editor. `handlers` is read at event time, so the
 *  parent can swap callbacks without rebuilding the editor. */
export function notesExtension(handlers: { current: NotesHandlers | null }): Extension {
  const decoField = StateField.define<DecorationSet>({
    create: (state) => decorations(state, handlers),
    update(deco, tr) {
      const changed = tr.effects.some((e) => e.is(setNoteMarks) || e.is(setNoteComposer)) || tr.docChanged
      return changed ? decorations(tr.state, handlers) : deco
    },
    provide: (f) => EditorView.decorations.from(f),
  })
  return [
    notesField,
    decoField,
    gutter({
      class: 'cm-k2-notes-gutter',
      markers(view) {
        const { marks } = view.state.field(notesField)
        const byLine = new Map<number, { count: number; outdated: boolean }>()
        for (const m of marks) {
          const l = lineOf(view.state, m.line)
          const cur = byLine.get(l) ?? { count: 0, outdated: true }
          byLine.set(l, { count: cur.count + 1, outdated: cur.outdated && m.outdated })
        }
        const b = new RangeSetBuilder<GutterMarker>()
        for (const l of [...byLine.keys()].sort((x, y) => x - y)) {
          const v = byLine.get(l)!
          b.add(view.state.doc.line(l).from, view.state.doc.line(l).from, new DotMarker(v.count, v.outdated))
        }
        return b.finish()
      },
      lineMarker(view, line) {
        const st = view.state.field(notesField)
        if (!st.canWrite) return null
        const n = view.state.doc.lineAt(line.from).number
        return st.marks.some((m) => lineOf(view.state, m.line) === n) ? null : ADD
      },
      lineMarkerChange: (u) => u.transactions.some((t) => t.effects.some((e) => e.is(setNoteMarks))),
      initialSpacer: () => ADD,
      domEventHandlers: {
        mousedown(view, line) {
          return startNoteAt(view, view.state.doc.lineAt(line.from).number)
        },
      },
    }),
    keymap.of([
      {
        key: 'c',
        run(view) {
          return startNoteAt(view, view.state.doc.lineAt(view.state.selection.main.head).number)
        },
      },
    ]),
    notesTheme,
  ]
}

const notesTheme = EditorView.theme({
  '.cm-k2-notes-gutter .cm-gutterElement': { cursor: 'pointer', padding: '0 3px', minWidth: '12px', textAlign: 'center' },
  '.cm-k2-note-add': { opacity: '0', color: 'var(--color-accent)', fontWeight: '600' },
  '.cm-k2-notes-gutter .cm-gutterElement:hover .cm-k2-note-add': { opacity: '1' },
  '.cm-k2-note-dot': { color: 'var(--color-accent)', fontSize: '9px' },
  '.cm-k2-note-dot-outdated': { color: 'var(--color-status-warn-text)' },
  '.cm-k2-note': {
    margin: '4px 8px 6px 8px',
    padding: '6px 8px',
    border: '1px solid var(--color-border)',
    borderLeft: '3px solid var(--color-accent)',
    background: 'var(--color-bg-elevated)',
    fontFamily: 'var(--font-sans, system-ui, sans-serif)',
    fontSize: '12px',
    whiteSpace: 'normal',
  },
  '.cm-k2-note[data-outdated]': { borderLeftColor: 'var(--color-status-warn-text)', opacity: '0.85' },
  '.cm-k2-note-head': { display: 'flex', alignItems: 'center', gap: '6px', color: 'var(--color-text-muted)', fontSize: '11px' },
  '.cm-k2-note-tag': { padding: '0 4px', border: '1px solid var(--color-border)', fontSize: '10px' },
  '.cm-k2-note-tag-outdated': { color: 'var(--color-status-warn-text)' },
  '.cm-k2-note-tools': { display: 'flex', gap: '6px', marginLeft: 'auto' },
  '.cm-k2-note-composer .cm-k2-note-tools': { marginTop: '4px', justifyContent: 'flex-end' },
  '.cm-k2-note-btn': { color: 'var(--color-text-muted)', cursor: 'pointer', background: 'none', border: 'none', fontSize: '11px' },
  '.cm-k2-note-btn:hover': { color: 'var(--color-text-primary)' },
  '.cm-k2-note-body': { marginTop: '3px', whiteSpace: 'pre-wrap', color: 'var(--color-text-primary)' },
  '.cm-k2-note-input': {
    width: '100%',
    marginTop: '4px',
    resize: 'vertical',
    background: 'var(--color-bg)',
    color: 'var(--color-text-primary)',
    border: '1px solid var(--color-border)',
    padding: '4px 6px',
    fontSize: '12px',
    fontFamily: 'inherit',
  },
  '.cm-k2-note-error': { color: 'var(--color-status-error-soft)', fontSize: '11px' },
})
