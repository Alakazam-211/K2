// The formatting toolbar's commands and the / menu's blocks (build-editors.md
// E3): pure transactions on the markdown text, so they work the same in a
// plain file and in a live doc (where each is one edit of yours). Pinned:
// what the caret's context turns on (Bold inside **…**, Heading on a heading
// line…), wrapping and unwrapping, Link selecting its URL to type over,
// Heading and List toggling per line, and the blocks the / menu inserts.

import { describe, expect, it } from 'vitest'
import { EditorSelection, EditorState } from '@codemirror/state'
import { ensureSyntaxTree } from '@codemirror/language'
import { gfmMarkdown } from './write-mode/language'
import { FORMAT_IDS, FORMAT_KEYS, formatSpec, formatState, type FormatId } from './md-commands'

function st(doc: string, from: number, to = from): EditorState {
  const s = EditorState.create({ doc, selection: EditorSelection.single(from, to), extensions: [gfmMarkdown()] })
  ensureSyntaxTree(s, doc.length, 5000)
  return s
}
const sel = (doc: string, word: string): [number, number] => [doc.indexOf(word), doc.indexOf(word) + word.length]
function run(s: EditorState, id: FormatId): { text: string; picked: string } {
  const spec = formatSpec(s, id)
  if (!spec) throw new Error(`${id} did nothing`)
  const next = s.update(spec).state
  const m = next.selection.main
  return { text: next.doc.toString(), picked: next.sliceDoc(m.from, m.to) }
}

describe('what the caret is in', () => {
  const DOC = '## Setup\n\nA **bold** and *slanted* and `code` and [link](x.md).\n\n- one\n- two'
  it('bold, italic, code and link from the spans around the caret', () => {
    expect(formatState(st(DOC, DOC.indexOf('bold') + 1))).toMatchObject({ bold: true, italic: false, code: false, link: false })
    expect(formatState(st(DOC, DOC.indexOf('slanted') + 2))).toMatchObject({ bold: false, italic: true })
    expect(formatState(st(DOC, DOC.indexOf('code') + 1))).toMatchObject({ code: true })
    expect(formatState(st(DOC, DOC.indexOf('link') + 1))).toMatchObject({ link: true })
  })
  it('heading and list from the line', () => {
    expect(formatState(st(DOC, 4))).toMatchObject({ heading: true, list: false })
    expect(formatState(st(DOC, DOC.indexOf('two')))).toMatchObject({ heading: false, list: true })
    expect(formatState(st(DOC, DOC.indexOf(' and ') + 1))).toEqual({ bold: false, italic: false, code: false, link: false, heading: false, list: false })
  })
})

describe('the formatting commands', () => {
  const P = 'Harbor turns field interviews into articles.'
  it('Bold wraps the selection in ** and keeps it selected; again unwraps', () => {
    const r = run(st(P, ...sel(P, 'field interviews')), 'bold')
    expect(r.text).toBe('Harbor turns **field interviews** into articles.')
    expect(r.picked).toBe('field interviews')
    const back = run(st(r.text, ...sel(r.text, 'field interviews')), 'bold')
    expect(back.text).toBe(P)
  })
  it('Italic uses *, Code uses backticks', () => {
    expect(run(st(P, ...sel(P, 'field')), 'italic').text).toBe('Harbor turns *field* interviews into articles.')
    expect(run(st(P, ...sel(P, 'field')), 'code').text).toBe('Harbor turns `field` interviews into articles.')
  })
  it('with no selection, Bold puts the caret between a new pair', () => {
    const s = st('ab', 1)
    const next = s.update(formatSpec(s, 'bold')!).state
    expect(next.doc.toString()).toBe('a****b')
    expect(next.selection.main.head).toBe(3)
  })
  it('Link wraps the words and selects "url" to type over; inside a link it unwraps', () => {
    const r = run(st(P, ...sel(P, 'articles')), 'link')
    expect(r.text).toBe('Harbor turns field interviews into [articles](url).')
    expect(r.picked).toBe('url')
    const L = 'See [the guide](docs/style.md) now.'
    expect(run(st(L, L.indexOf('guide')), 'link').text).toBe('See the guide now.')
  })
  it('Heading makes the line an H2, and takes it off again', () => {
    expect(run(st(P, 3), 'heading').text).toBe(`## ${P}`)
    expect(run(st(`## ${P}`, 6), 'heading').text).toBe(P)
    expect(run(st(`# ${P}`, 6), 'heading').text).toBe(`## ${P}`)
  })
  it('List puts "- " on every selected line, or takes it off when they all have it', () => {
    const D = 'one\ntwo\nthree'
    expect(run(st(D, 0, D.length), 'list').text).toBe('- one\n- two\n- three')
    expect(run(st('- one\n- two', 0, 11), 'list').text).toBe('one\ntwo')
  })
  it('read only: nothing', () => {
    const s = EditorState.create({ doc: P, selection: EditorSelection.single(0, 6), extensions: [gfmMarkdown(), EditorState.readOnly.of(true)] })
    expect(formatSpec(s, 'bold')).toBeNull()
  })
})

describe('keys', () => {
  it('every button has a key, shown as the title shows it', () => {
    expect(FORMAT_IDS).toEqual(['bold', 'italic', 'code', 'link', 'heading', 'list'])
    expect(FORMAT_IDS.map((id) => FORMAT_KEYS[id].label)).toEqual(['⌘B', '⌘I', '⇧⌘C', '⌘K', '⌥⌘2', '⇧⌘8'])
  })
})
