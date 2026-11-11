// @vitest-environment jsdom
//
// The legacy terminal view: the text a file drop or a Finder paste writes
// to its PTY. Built by lib/drop-payload.ts (tested there); this pins that
// the view uses it and writes nothing after a refusal. Only the view's I/O
// boundaries are mocked.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  writes: [] as Array<{ id: string; data: string }>,
  clipboardPaths: [] as string[],
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalExists: vi.fn(async () => true),
  terminalGetGrid: vi.fn(async () => ({ lines: [], cursor: { row: 0, col: 0, visible: true }, cols: 80, rows: 24 })),
  terminalCreate: vi.fn(async () => undefined),
  terminalResize: vi.fn(async () => undefined),
  terminalScroll: vi.fn(async () => undefined),
  terminalSetFocus: vi.fn(async () => undefined),
  terminalWrite: vi.fn(async (_scope: unknown, id: string, data: string) => {
    h.writes.push({ id, data })
  }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, path: string) =>
    path === 'fs/clipboard-paths' ? h.clipboardPaths : undefined,
  ),
}))
vi.mock('./TerminalComposeBar', () => ({ TerminalComposeBar: () => null }))

import { AlacrittyTerminalView } from './AlacrittyTerminalView'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { useToastStore } from '@/stores/toast'
import { CONTROL_CHAR_REFUSAL } from '@/lib/drop-payload'

const room = testRoom({ tabs: {} })

async function view(): Promise<HTMLElement> {
  renderInRoom(room, <AlacrittyTerminalView terminalId="term-1" cwd="/tmp/ws" />)
  const el = document.querySelector<HTMLElement>('[data-terminal-container]')
  if (!el) throw new Error('no terminal container')
  // The PTY id is set once the existing terminal is attached.
  const { terminalGetGrid } = await import('@/lib/terminal-daemon')
  await waitFor(() => expect(terminalGetGrid).toHaveBeenCalled())
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
  return el
}

function dropFiles(el: HTMLElement, paths: string[]): void {
  fireEvent.drop(el, { dataTransfer: { files: paths.map((path) => ({ path })) } })
}

async function pasteFinderPaths(el: HTMLElement, paths: string[]): Promise<void> {
  h.clipboardPaths = paths
  await act(async () => {
    fireEvent.paste(el, { clipboardData: { getData: () => 'ignored' } })
    await new Promise((r) => setTimeout(r, 0))
  })
}

const toasts = () => useToastStore.getState().toasts.map((t) => [t.message, t.type])

class StubResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', StubResizeObserver)
  h.writes = []
  h.clipboardPaths = []
  useToastStore.setState({ toasts: [] })
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('AlacrittyTerminalView: files dropped or pasted', () => {
  it('a dropped file is written backslash-escaped with a trailing space', async () => {
    const el = await view()
    dropFiles(el, ['/tmp/my notes.txt'])
    expect(h.writes).toEqual([{ id: 'term-1', data: '/tmp/my\\ notes.txt ' }])
  })

  it('a dropped name with a line break: a toast, and nothing written', async () => {
    const el = await view()
    dropFiles(el, ['/tmp/notes\nrm -rf ~'])
    expect(h.writes).toEqual([])
    expect(toasts()).toEqual([[CONTROL_CHAR_REFUSAL, 'warning']])
  })

  it('Finder paths pasted: the same text as a drop; a line break is refused', async () => {
    const el = await view()
    await pasteFinderPaths(el, ['/tmp/a b.txt'])
    expect(h.writes).toEqual([{ id: 'term-1', data: '/tmp/a\\ b.txt ' }])

    await pasteFinderPaths(el, ['/tmp/x\ny.txt'])
    expect(h.writes).toEqual([{ id: 'term-1', data: '/tmp/a\\ b.txt ' }])
    expect(toasts()).toEqual([[CONTROL_CHAR_REFUSAL, 'warning']])
  })
})
