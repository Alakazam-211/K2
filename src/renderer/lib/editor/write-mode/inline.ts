// Write mode's line and inline decorations (build-editors.md E2), for the
// visible part of the file only. A line the caret (or a selection) is on
// shows its marks in the muted ink; every other line hides them and renders:
// heading sizes, bold, italic, code, links, quotes, rules, list bullets that
// hang in the margin, task checkboxes and code fences as cards. A list or
// quote marker hangs to the left of the text in both states, so revealing it
// never moves the words. Tables and images come from ./blocks.

import { type EditorState, type Extension, Prec, type Range } from '@codemirror/state'
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from '@codemirror/view'
import { syntaxTree } from '@codemirror/language'
import type { SyntaxNode, SyntaxNodeRef } from '@lezer/common'
import { fenceLabel, fenceLanguage } from './language'
import { revealField, revealedAt, revealedIn, type LineSpan } from './reveal'
import { BulletWidget, CheckboxWidget, FenceHeaderWidget } from './widgets'
import { linkHref } from './table'

const HIDE = Decoration.replace({})
const MUTED = Decoration.mark({ class: 'cm-md-mark' })
const markCache = new Map<string, Decoration>()
const mark = (cls: string): Decoration => {
  let d = markCache.get(cls)
  if (!d) markCache.set(cls, (d = Decoration.mark({ class: cls })))
  return d
}
const lineCache = new Map<string, Decoration>()
const line = (cls: string, ind?: number, rows?: string): Decoration => {
  const key = `${cls}|${ind ?? ''}|${rows ?? ''}`
  let d = lineCache.get(key)
  if (!d) {
    const attributes: Record<string, string> = { class: cls }
    if (ind !== undefined) attributes.style = `--ind: ${ind}`
    if (rows) attributes['data-k2-chamfer-rows'] = rows
    lineCache.set(key, (d = Decoration.line({ attributes })))
  }
  return d
}
/** A fence row's place in its card, for Terminal's cut corners. */
const rowsOf = (top: boolean, end: boolean): string | undefined => (top && end ? 'only' : top ? 'first' : end ? 'last' : undefined)
const MOD = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl'

const LIST = new Set(['BulletList', 'OrderedList'])
const CODE = new Set(['FencedCode', 'CodeBlock'])

const HANG = Decoration.mark({ class: 'cm-md-lm' })

class Builder {
  readonly out: Range<Decoration>[] = []
  /** Hanging markers: one outer span each (lowest precedence), so the syntax
   *  colours' spans sit inside it instead of splitting it into pieces. */
  readonly hang: Range<Decoration>[] = []
  readonly code: Array<[number, number]> = []
  /** A revealed table: every line in it shows its marks. */
  forced: [number, number] | null = null
  constructor(readonly state: EditorState, readonly reveal: readonly LineSpan[], readonly read: boolean) {}

  revealed(pos: number): boolean {
    if (this.read) return false
    if (this.forced && pos >= this.forced[0] && pos <= this.forced[1]) return true
    return revealedAt(this.reveal, pos)
  }
  add(d: Decoration, from: number, to = from): void {
    this.out.push(d.range(from, to))
  }
  /** A mark: muted where revealed, hidden elsewhere (with the space after it when `space`). */
  marker(from: number, to: number, space = false): void {
    if (to <= from) return
    if (this.revealed(from)) return this.add(MUTED, from, to)
    const end = space && this.state.sliceDoc(to, to + 1) === ' ' ? to + 1 : to
    this.add(HIDE, from, end)
  }
  /** A marker hung left of the text: muted where revealed, else `ink` (or hidden when there's none). */
  hung(from: number, to: number, ink: Decoration | null): void {
    if (to <= from) return
    if (!this.revealed(from) && !ink) return this.add(HIDE, from, to)
    this.hang.push(HANG.range(from, to))
    this.add(this.revealed(from) ? MUTED : ink!, from, to)
  }
  /** The lines of [from, to] inside the visible span. */
  *lines(from: number, to: number, vis: [number, number]): Generator<{ from: number; to: number; number: number }> {
    const doc = this.state.doc
    let l = doc.lineAt(Math.max(from, vis[0]))
    const end = Math.min(to, vis[1])
    for (;;) {
      yield l
      if (l.to >= end || l.number >= doc.lines) break
      l = doc.line(l.number + 1)
    }
  }
}

/** How many lists `node` sits inside, minus one: 0 for a top-level item. */
function listDepth(node: SyntaxNode): number {
  let d = -1
  for (let p: SyntaxNode | null = node; p; p = p.parent) if (LIST.has(p.name)) d++
  return Math.max(0, d)
}

function listItem(b: Builder, markNode: SyntaxNode): void {
  const { state } = b
  const item = markNode.parent
  if (!item) return
  const l = state.doc.lineAt(markNode.from)
  if (/\S/.test(state.sliceDoc(l.from, markNode.from).replace(/>/g, ''))) return
  const ordered = item.parent?.name === 'OrderedList'
  const task = item.getChild('Task')?.getChild('TaskMarker') ?? null
  const markEnd = task ? task.to : markNode.to
  const start = Math.min(l.to, state.sliceDoc(markEnd, markEnd + 1) === ' ' ? markEnd + 1 : markEnd)
  const depth = listDepth(markNode)
  b.add(line(task ? 'cm-md-li cm-md-task' : 'cm-md-li', depth), l.from)
  if (task && /x/i.test(state.sliceDoc(task.from, task.to)) && start < l.to) b.add(mark('cm-md-done'), start, l.to)
  if (b.revealed(l.from) || ordered) return b.hung(l.from, start, mark('cm-md-num'))
  if (task) b.add(Decoration.replace({ widget: new CheckboxWidget(/x/i.test(state.sliceDoc(task.from, task.to)), b.read) }), l.from, start)
  else b.add(Decoration.replace({ widget: new BulletWidget(depth) }), l.from, start)
}

/** Lines of a list item after its first: the same hanging indent, the leading spaces hung too. */
function listContinuation(b: Builder, node: SyntaxNode, vis: [number, number]): void {
  const item = node.parent
  if (!item || item.name !== 'ListItem') return
  const first = b.state.doc.lineAt(item.from).number
  const task = item.getChild('Task') !== null
  const depth = listDepth(node)
  for (const l of b.lines(node.from, node.to, vis)) {
    if (l.number === first) continue
    b.add(line(task ? 'cm-md-li cm-md-task' : 'cm-md-li', depth), l.from)
    const lead = /^\s*/.exec(b.state.sliceDoc(l.from, l.to))![0].length
    b.hung(l.from, l.from + lead, null)
  }
}

function quote(b: Builder, node: SyntaxNode, vis: [number, number]): void {
  for (const l of b.lines(node.from, node.to, vis)) {
    b.add(line('cm-md-quote'), l.from)
    const text = b.state.sliceDoc(l.from, l.to)
    const prefix = /^\s{0,3}(?:>\s?)+/.exec(text)?.[0].length ?? 0
    b.hung(l.from, l.from + prefix, null)
    if (l.to > l.from + prefix) b.add(mark('cm-md-quote-text'), l.from + prefix, l.to)
  }
}

function fence(b: Builder, node: SyntaxNode, vis: [number, number]): void {
  const doc = b.state.doc
  const first = doc.lineAt(node.from)
  const last = doc.lineAt(node.to)
  b.code.push([first.from, last.to])
  if (node.name === 'CodeBlock') {
    for (const l of b.lines(node.from, node.to, vis)) {
      const top = l.number === first.number
      const end = l.number === last.number
      b.add(line(`cm-md-fence${top ? ' cm-md-fence-top' : ''}${end ? ' cm-md-fence-end' : ''}`, undefined, rowsOf(top, end)), l.from)
    }
    return
  }
  const closer = node.lastChild?.name === 'CodeMark' && node.lastChild.from > first.to ? node.lastChild : null
  const info = node.getChild('CodeInfo')
  const lang = info ? b.state.sliceDoc(info.from, info.to) : ''
  const plain = !fenceLanguage(lang)
  for (const l of b.lines(node.from, node.to, vis)) {
    const top = l.number === first.number
    const end = l.number === last.number
    b.add(line(`cm-md-fence${top ? ' cm-md-fence-top cm-md-fence-open' : ''}${end ? ' cm-md-fence-end' : ''}${end && closer ? ' cm-md-fence-close' : ''}`, undefined, rowsOf(top, end)), l.from)
    if (top) {
      if (b.revealed(l.from)) {
        if (l.to > l.from) b.add(MUTED, l.from, l.to)
      } else b.add(Decoration.replace({ widget: new FenceHeaderWidget(fenceLabel(lang)) }), l.from, l.to)
    } else if (end && closer) {
      if (l.to > l.from) b.add(b.revealed(l.from) ? MUTED : HIDE, l.from, l.to)
    } else if (plain && l.to > l.from) b.add(mark('cm-md-plaincode'), l.from, l.to)
  }
}

/** A link or image: its text styled, the brackets and destination hidden or muted. */
function link(b: Builder, node: SyntaxNode, cls: string): void {
  const marks = node.getChildren('LinkMark')
  if (marks.length < 2) return
  const open = marks[0]
  const close = marks[1]
  const href = linkHref(b.state, node)
  if (close.from > open.to) {
    if (cls === 'cm-md-link') {
      const title = href ? (b.read ? href : `${href} · ${MOD}-click to open`) : ''
      b.add(Decoration.mark({ class: cls, attributes: href ? { 'data-href': href, title } : {} }), open.to, close.from)
    } else b.add(mark(cls), open.to, close.from)
  }
  b.marker(open.from, open.to)
  b.marker(close.from, node.to)
}

function heading(b: Builder, node: SyntaxNodeRef): void {
  const doc = b.state.doc
  const level = Number(node.name.slice(-1))
  const cls = `cm-md-h${Math.min(level, 4)}`
  if (node.name.startsWith('ATX')) {
    b.add(line(cls), doc.lineAt(node.from).from)
    return
  }
  // Setext: the text lines are the heading; the underline is its mark.
  const last = doc.lineAt(node.to)
  for (let n = doc.lineAt(node.from).number; n < last.number; n++) b.add(line(cls), doc.line(n).from)
  b.add(line('cm-md-setext-rule'), last.from)
}

function headerMark(b: Builder, node: SyntaxNodeRef): void {
  const l = b.state.doc.lineAt(node.from)
  if (node.node.parent?.name.startsWith('Setext')) return b.marker(node.from, node.to)
  const leading = !/\S/.test(b.state.sliceDoc(l.from, node.from))
  if (leading) return b.marker(node.from, node.to, true)
  // A closing run of #s, with the space before it.
  const from = b.state.sliceDoc(node.from - 1, node.from) === ' ' && !b.revealed(l.from) ? node.from - 1 : node.from
  b.marker(from, node.to)
}

export interface InlineDecorations {
  marks: DecorationSet
  hang: DecorationSet
}

/** The decorations for the visible part of the file. */
export function buildInline(state: EditorState, vis: [number, number], read: boolean): InlineDecorations {
  const reveal = read ? [] : state.field(revealField).lines
  const b = new Builder(state, reveal, read)
  const doc = state.doc
  syntaxTree(state).iterate({
    from: vis[0],
    to: vis[1],
    enter: (n) => {
      switch (n.name) {
        case 'ATXHeading1': case 'ATXHeading2': case 'ATXHeading3': case 'ATXHeading4': case 'ATXHeading5': case 'ATXHeading6':
        case 'SetextHeading1': case 'SetextHeading2':
          heading(b, n)
          return
        case 'HeaderMark':
          return headerMark(b, n)
        case 'Emphasis': b.add(mark('cm-md-em'), n.from, n.to); return
        case 'StrongEmphasis': b.add(mark('cm-md-strong'), n.from, n.to); return
        case 'Strikethrough': b.add(mark('cm-md-strike'), n.from, n.to); return
        case 'EmphasisMark': case 'StrikethroughMark':
          return b.marker(n.from, n.to)
        case 'InlineCode': {
          const marks = n.node.getChildren('CodeMark')
          if (marks.length === 2) {
            if (marks[1].from > marks[0].to) b.add(mark('cm-md-code'), marks[0].to, marks[1].from)
            b.marker(marks[0].from, marks[0].to)
            b.marker(marks[1].from, marks[1].to)
          }
          return false
        }
        case 'Link': link(b, n.node, 'cm-md-link'); return
        case 'Image': link(b, n.node, 'cm-md-caption'); return false
        case 'Autolink': case 'URL': {
          const parent = n.node.parent?.name
          if (n.name === 'URL' && (parent === 'Link' || parent === 'Image' || parent === 'Autolink')) return
          const href = state.sliceDoc(n.from, n.to).replace(/^<|>$/g, '')
          const title = b.read ? href : `${href} · ${MOD}-click to open`
          const inner = n.name === 'Autolink' ? [n.from + 1, n.to - 1] : [n.from, n.to]
          b.add(Decoration.mark({ class: 'cm-md-link', attributes: { 'data-href': href, title } }), inner[0], inner[1])
          if (n.name === 'Autolink') {
            b.marker(n.from, n.from + 1)
            b.marker(n.to - 1, n.to)
          }
          return false
        }
        case 'LinkMark': case 'LinkTitle': case 'LinkLabel': case 'QuoteMark': case 'TaskMarker':
          return
        case 'Escape':
          if (!b.revealed(n.from)) b.add(HIDE, n.from, n.from + 1)
          return
        case 'Blockquote': quote(b, n.node, vis); return
        case 'ListMark': listItem(b, n.node); return
        case 'Paragraph': case 'Task': listContinuation(b, n.node, vis); return
        case 'FencedCode': case 'CodeBlock': fence(b, n.node, vis); return false
        case 'HorizontalRule': {
          const l = doc.lineAt(n.from)
          b.add(line('cm-md-hr'), l.from)
          b.add(b.revealed(l.from) ? MUTED : HIDE, n.from, n.to)
          return
        }
        case 'Table': {
          const a = doc.lineAt(n.from).from
          const z = doc.lineAt(n.to).to
          // Not revealed: ./blocks draws it.
          if (read || !revealedIn(reveal, a, z)) return false
          b.forced = [a, z]
          for (const l of b.lines(a, z, vis)) b.add(line('cm-md-table-src'), l.from)
          return
        }
        case 'HTMLBlock': case 'CommentBlock': case 'ProcessingInstructionBlock':
          return false
      }
      return undefined
    },
    leave: (n) => {
      if (n.name === 'Table') b.forced = null
    },
  })
  // Blank lines between blocks are short, as a rendered page's gaps are; the caret's stays full height
  // (a selection running over a blank line leaves it short, so the page doesn't jump as it grows).
  const heads = read ? [] : state.selection.ranges.map((r) => doc.lineAt(r.head).from)
  for (const l of b.lines(vis[0], vis[1], vis)) {
    if (l.to > l.from && /\S/.test(doc.sliceString(l.from, l.to))) continue
    if (b.code.some(([a, z]) => l.from >= a && l.from <= z) || (b.revealed(l.from) && heads.includes(l.from))) continue
    b.add(line('cm-md-blank'), l.from)
  }
  return { marks: Decoration.set(b.out, true), hang: Decoration.set(b.hang, true) }
}

/** The visible span: one range from the first visible line to the last. */
function visibleSpan(view: EditorView): [number, number] {
  const r = view.visibleRanges
  return r.length ? [r[0].from, r[r.length - 1].to] : [view.viewport.from, view.viewport.to]
}

/** The plugin, at highest precedence (its inks sit inside the syntax colours'),
 *  plus its hanging markers at lowest (outside them). */
export function inlineDecorations(read: boolean): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      d: InlineDecorations
      constructor(view: EditorView) {
        this.d = buildInline(view.state, visibleSpan(view), read)
      }
      update(u: ViewUpdate): void {
        const reveal = u.state.field(revealField).lines !== u.startState.field(revealField).lines
        if (u.docChanged || u.viewportChanged || reveal || syntaxTree(u.state) !== syntaxTree(u.startState)) {
          this.d = buildInline(u.state, visibleSpan(u.view), read)
        }
      }
    },
    { decorations: (v) => v.d.marks },
  )
  return [Prec.highest(plugin), Prec.lowest(EditorView.decorations.of((view) => view.plugin(plugin)?.d.hang ?? Decoration.none))]
}
