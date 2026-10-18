// Thread file links: a click opens the file the way the Files drawer does,
// in the Thread's room, and hands the editor / tree a one-shot request.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const activateTab = vi.fn()
vi.mock('@/stores/panels', () => ({ usePanelsStore: { getState: () => ({ activateTab }) } }))

import { openThreadFileLink } from './thread-file-link-open'
import { lineColToPos, onFileLineRequest, onFileTreeReveal, takeFileLine, takeFileTreeReveal } from './file-open-requests'
import { Text } from '@codemirror/state'

function fakeRoom(isPrimary = true) {
  const openFileAsTab = vi.fn()
  return {
    room: { scope: { hostKey: 'local' }, tabs: { getState: () => ({ openFileAsTab }) }, isPrimary } as never,
    openFileAsTab,
  }
}

beforeEach(() => {
  activateTab.mockClear()
  vi.useRealTimers()
})
afterEach(() => vi.useRealTimers())

describe('openThreadFileLink', () => {
  it('a file opens as a file tab in the room and asks for its line', () => {
    const { room, openFileAsTab } = fakeRoom()
    const heard = vi.fn()
    const off = onFileLineRequest('local', '/ws/src/a.ts', heard)
    openThreadFileLink(room, { input: 'src/a.ts', linkable: true, kind: 'file', rel: 'src/a.ts', path: '/ws/src/a.ts' }, 12, 3)
    off()
    expect(openFileAsTab).toHaveBeenCalledWith('/ws/src/a.ts')
    expect(heard).toHaveBeenCalledTimes(1)
    expect(takeFileLine('local', '/ws/src/a.ts')).toEqual({ line: 12, col: 3 })
    // One shot.
    expect(takeFileLine('local', '/ws/src/a.ts')).toBeNull()
    expect(activateTab).not.toHaveBeenCalled()
  })

  it('a file without a line leaves no line request', () => {
    const { room, openFileAsTab } = fakeRoom()
    openThreadFileLink(room, { input: 'b.ts', linkable: true, kind: 'file', rel: 'b.ts', path: '/ws/b.ts' })
    expect(openFileAsTab).toHaveBeenCalledWith('/ws/b.ts')
    expect(takeFileLine('local', '/ws/b.ts')).toBeNull()
  })

  it('a folder opens the Files drawer and reveals it there', () => {
    const { room, openFileAsTab } = fakeRoom()
    const heard = vi.fn()
    const off = onFileTreeReveal('local', heard)
    openThreadFileLink(room, { input: 'src/', linkable: true, kind: 'directory', rel: 'src', path: '/ws/src' })
    off()
    expect(openFileAsTab).not.toHaveBeenCalled()
    expect(activateTab).toHaveBeenCalledWith('files')
    expect(heard).toHaveBeenCalledTimes(1)
    // Another tree's root does not take it; the right one does, once.
    expect(takeFileTreeReveal('local', '/other')).toBeNull()
    expect(takeFileTreeReveal('local', '/ws')).toBe('/ws/src')
    expect(takeFileTreeReveal('local', '/ws')).toBeNull()
  })

  it('a folder in a Home room does not touch the window drawers', () => {
    const { room } = fakeRoom(false)
    openThreadFileLink(room, { input: 'src/', linkable: true, kind: 'directory', rel: 'src', path: '/ws/src' })
    expect(activateTab).not.toHaveBeenCalled()
    expect(takeFileTreeReveal('local', '/ws')).toBe('/ws/src')
  })
})

describe('file-open-requests', () => {
  it('requests expire, so a jump nobody took never fires later', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-10-18T12:00:00Z'))
    const { room } = fakeRoom()
    openThreadFileLink(room, { input: 'a.md', linkable: true, kind: 'file', rel: 'a.md', path: '/ws/a.md' }, 4)
    vi.setSystemTime(new Date('2026-10-18T12:00:06Z'))
    expect(takeFileLine('local', '/ws/a.md')).toBeNull()
  })

  it('requests are per server', () => {
    const { room } = fakeRoom()
    openThreadFileLink(room, { input: 'a.ts', linkable: true, kind: 'file', rel: 'a.ts', path: '/ws/a.ts' }, 2)
    expect(takeFileLine('host:other', '/ws/a.ts')).toBeNull()
    expect(takeFileLine('local', '/ws/a.ts')).toEqual({ line: 2 })
  })
})

describe('lineColToPos', () => {
  const doc = Text.of(['first', 'second line', 'x'])
  it('maps 1-based line and column to an offset', () => {
    expect(lineColToPos(doc, 1)).toBe(0)
    expect(lineColToPos(doc, 2)).toBe(6)
    expect(lineColToPos(doc, 2, 3)).toBe(8)
  })
  it('clamps past the end of the line and the doc', () => {
    expect(lineColToPos(doc, 2, 99)).toBe(6 + 'second line'.length)
    expect(lineColToPos(doc, 99)).toBe(doc.line(3).from)
    expect(lineColToPos(doc, 0)).toBe(0)
  })
})
