// @vitest-environment jsdom
//
// Image drop into a terminal. Claude Code shows `[Image #N]` only when the
// pasted text is a path it can fs.exists(), so image paths are quoted (never
// backslash-escaped) and the whole payload is a bracketed paste, which is
// the paste event Claude's image detector reads. These pin the in-app drag
// from the file tree onto a terminal (the helpers: drop-payload.test.ts).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  writes: [] as Array<{ scope: string; id: string; data: string }>,
  osDrags: [] as unknown[],
}))

vi.mock('@crabnebula/tauri-plugin-drag', () => ({
  startDrag: vi.fn(async (opts: unknown) => {
    h.osDrags.push(opts)
  }),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalWrite: vi.fn(async (scope: { id: string }, id: string, data: string) => {
    h.writes.push({ scope: scope.id, id, data })
  }),
}))

import { beginFileDrag, isFileDragActive } from './file-drag'
import { bracketPaste } from './drop-payload'
import { fakeScope } from '@/test-utils/fake-scope'

describe('in-app drag from the file tree onto a terminal', () => {
  const scope = fakeScope('local', { remote: false })
  let target: HTMLElement

  /** A terminal container like TerminalPane's (v2) or Alacritty's (v1). */
  function mountTerminal(kind: 'v1' | 'v2'): HTMLElement {
    const term = document.createElement('div')
    term.dataset.terminalId = 'term-1'
    if (kind === 'v2') term.dataset.terminalKind = 'v2'
    const inner = document.createElement('canvas')
    term.appendChild(inner)
    document.body.appendChild(term)
    return inner
  }

  function drop(paths: string[]): void {
    beginFileDrag({ scope, mayHandOffToOs: true }, paths, 10, 10)
    expect(isFileDragActive()).toBe(true)
    document.dispatchEvent(new MouseEvent('mouseup', { clientX: 40, clientY: 40 }))
    expect(isFileDragActive()).toBe(false)
  }

  beforeEach(() => {
    h.writes = []
    h.osDrags = []
    document.elementFromPoint = vi.fn(() => target)
  })
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('v2 terminal: an image goes as a bracketed paste of the quoted path', () => {
    target = mountTerminal('v2')
    const seen: string[] = []
    target.parentElement!.addEventListener('k2so:terminal-write', (e) => {
      seen.push((e as CustomEvent<{ data: string }>).detail.data)
    })
    drop(['/Users/a/Screen Shot.png', '/Users/a/notes.txt'])
    expect(seen).toEqual([bracketPaste("'/Users/a/Screen Shot.png' /Users/a/notes.txt")])
    expect(h.writes).toEqual([])
  })

  it('v2 terminal: no image, no bracketed paste', () => {
    target = mountTerminal('v2')
    const seen: string[] = []
    target.parentElement!.addEventListener('k2so:terminal-write', (e) => {
      seen.push((e as CustomEvent<{ data: string }>).detail.data)
    })
    drop(['/Users/a/notes.txt', '/Users/a/b.md'])
    expect(seen).toEqual(['/Users/a/notes.txt /Users/a/b.md'])
  })

  it('v1 terminal: the same payload is written to that terminal on the source server', () => {
    target = mountTerminal('v1')
    drop(['/tmp/shot.heic'])
    expect(h.writes).toEqual([{ scope: scope.id, id: 'term-1', data: bracketPaste('/tmp/shot.heic') }])
    expect(h.osDrags).toEqual([])
  })
})
