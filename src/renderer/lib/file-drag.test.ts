// @vitest-environment jsdom
//
// Image drop into a terminal. Claude Code shows `[Image #N]` only when the
// pasted text is a path it can fs.exists(), so image paths are quoted (never
// backslash-escaped) and the whole payload is a bracketed paste, which is
// the paste event Claude's image detector reads. These pin the helpers and
// the in-app drag from the file tree onto a terminal.

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

import {
  BRACKETED_PASTE_END,
  BRACKETED_PASTE_START,
  beginFileDrag,
  bracketPaste,
  isFileDragActive,
  isImagePath,
  quotePathForImageDrop,
} from './file-drag'
import { fakeScope } from '@/test-utils/fake-scope'

describe('isImagePath', () => {
  it('knows the image and PDF extensions Claude Code reads, in any case', () => {
    for (const ext of ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'heic', 'heif', 'pdf']) {
      expect(isImagePath(`/tmp/shot.${ext}`), ext).toBe(true)
      expect(isImagePath(`/tmp/SHOT.${ext.toUpperCase()}`), ext.toUpperCase()).toBe(true)
    }
  })

  it('goes by the last extension only', () => {
    expect(isImagePath('/tmp/notes.txt')).toBe(false)
    expect(isImagePath('/tmp/shot.png.txt')).toBe(false)
    expect(isImagePath('/tmp/png')).toBe(false)
    expect(isImagePath('/tmp/archive.tar.gz')).toBe(false)
  })
})

describe('quotePathForImageDrop', () => {
  it('leaves a plain path bare', () => {
    expect(quotePathForImageDrop('/tmp/shot-1_a.png')).toBe('/tmp/shot-1_a.png')
  })

  it('single-quotes a path with spaces and keeps it literal (no backslashes)', () => {
    expect(quotePathForImageDrop('/Users/a/Screen Shot 2026.png')).toBe("'/Users/a/Screen Shot 2026.png'")
  })

  it("closes and reopens the quotes around an apostrophe", () => {
    expect(quotePathForImageDrop("/tmp/Bob's shot.png")).toBe("'/tmp/Bob'\\''s shot.png'")
  })
})

describe('bracketPaste', () => {
  it('wraps the text in the bracketed-paste start and end sequences', () => {
    expect(BRACKETED_PASTE_START).toBe('\x1b[200~')
    expect(BRACKETED_PASTE_END).toBe('\x1b[201~')
    expect(bracketPaste('/tmp/a.png ')).toBe('\x1b[200~/tmp/a.png \x1b[201~')
  })
})

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
