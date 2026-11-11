// @vitest-environment jsdom
//
// In-app drag from the file tree onto a terminal or a compose bar. The text
// is the same as an external drop's (lib/drop-payload.ts, tested there):
// non-image paths backslash-escaped, image paths quoted and the whole payload
// a bracketed paste so Claude Code shows `[Image #N]`. A name with a control
// character is refused with a toast and nothing is written.

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
import { CONTROL_CHAR_REFUSAL, bracketPaste } from './drop-payload'
import { COMPOSE_BAR_SELECTOR } from './compose-surface-drop'
import { useToastStore } from '@/stores/toast'
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
    expect(seen).toEqual([bracketPaste("'/Users/a/Screen Shot.png' /Users/a/notes.txt ")])
    expect(h.writes).toEqual([])
  })

  it('v2 terminal: no image, no bracketed paste', () => {
    target = mountTerminal('v2')
    const seen: string[] = []
    target.parentElement!.addEventListener('k2so:terminal-write', (e) => {
      seen.push((e as CustomEvent<{ data: string }>).detail.data)
    })
    drop(['/Users/a/my notes.txt', '/Users/a/b.md'])
    expect(seen).toEqual(['/Users/a/my\\ notes.txt /Users/a/b.md '])
  })

  it('v1 terminal: the same payload is written to that terminal on the source server', () => {
    target = mountTerminal('v1')
    drop(['/tmp/shot.heic'])
    expect(h.writes).toEqual([{ scope: scope.id, id: 'term-1', data: bracketPaste('/tmp/shot.heic ') }])
    expect(h.osDrags).toEqual([])
  })

  it('a name with a line break: a toast, and nothing reaches a v2 or v1 terminal', () => {
    for (const kind of ['v2', 'v1'] as const) {
      useToastStore.setState({ toasts: [] })
      target = mountTerminal(kind)
      const seen: string[] = []
      target.parentElement!.addEventListener('k2so:terminal-write', (e) => {
        seen.push((e as CustomEvent<{ data: string }>).detail.data)
      })
      drop(['/tmp/notes\nrm -rf ~'])
      expect(seen, kind).toEqual([])
      expect(h.writes, kind).toEqual([])
      expect(useToastStore.getState().toasts.map((t) => [t.message, t.type]), kind).toEqual([
        [CONTROL_CHAR_REFUSAL, 'warning'],
      ])
      document.body.innerHTML = ''
    }
  })

  it('Windows paths: PowerShell quoting for v2, cmd quoting for v1', () => {
    target = mountTerminal('v2')
    const seen: string[] = []
    target.parentElement!.addEventListener('k2so:terminal-write', (e) => {
      seen.push((e as CustomEvent<{ data: string }>).detail.data)
    })
    drop(['C:\\Users\\a\\my notes.txt'])
    expect(seen).toEqual(["'C:\\Users\\a\\my notes.txt' "])
    document.body.innerHTML = ''

    target = mountTerminal('v1')
    drop(['C:\\Users\\a\\my notes.txt'])
    expect(h.writes).toEqual([{ scope: scope.id, id: 'term-1', data: '"C:\\Users\\a\\my notes.txt" ' }])
  })
})

describe('in-app drag from the file tree onto a compose bar', () => {
  const scope = fakeScope('local', { remote: false })
  let bar: HTMLElement
  let seen: string[]

  function drop(paths: string[]): void {
    beginFileDrag({ scope, mayHandOffToOs: true }, paths, 10, 10)
    document.dispatchEvent(new MouseEvent('mouseup', { clientX: 40, clientY: 40 }))
  }

  beforeEach(() => {
    h.writes = []
    useToastStore.setState({ toasts: [] })
    const holder = document.createElement('div')
    holder.innerHTML = '<div data-compose-bar=""><textarea></textarea></div>'
    document.body.appendChild(holder)
    bar = holder.firstElementChild as HTMLElement
    expect(bar.matches(COMPOSE_BAR_SELECTOR)).toBe(true)
    seen = []
    bar.addEventListener('k2so:compose-insert', (e) => {
      seen.push((e as CustomEvent<{ data: string }>).detail.data)
    })
    const inner = bar.querySelector('textarea') as HTMLElement
    document.elementFromPoint = vi.fn(() => inner)
  })
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it("inserts the same text as an external drop: my\\ notes.txt, not 'my notes.txt'", () => {
    drop(['/Users/a/my notes.txt', '/Users/a/Screen Shot.png'])
    expect(seen).toEqual(["/Users/a/my\\ notes.txt '/Users/a/Screen Shot.png' "])
  })

  it('a name with a line break: a toast, and no insert', () => {
    drop(['/Users/a/x\ny.txt'])
    expect(seen).toEqual([])
    expect(useToastStore.getState().toasts.map((t) => t.message)).toEqual([CONTROL_CHAR_REFUSAL])
  })
})
