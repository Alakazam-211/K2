import { describe, it, expect, vi, beforeEach } from 'vitest'
import { isComposePreviewImagePath } from '@/components/Terminal/terminalCompose'
import {
  COMPOSE_BAR_SELECTOR,
  COMPOSE_DROP_SURFACE_SELECTOR,
  filesAreComposeSurfaceImages,
  isComposeSurfaceImageFile,
  isComposeSurfaceImagePath,
  pathsAreComposeSurfaceImages,
} from './compose-surface-drop'
import {
  BRACKETED_PASTE_START,
  formatInAppComposeInsert,
  inAppComposeDropElement,
} from './file-drag'

const dropMocks = vi.hoisted(() => ({
  executeBrowserFileDrop: vi.fn(async (..._args: unknown[]) => '/host/shot.png '),
  executeRemoteDrop: vi.fn(async (..._args: unknown[]) => null as string | null),
}))

vi.mock('./handle-remote-drop', () => ({
  executeBrowserFileDrop: (...args: unknown[]) => dropMocks.executeBrowserFileDrop(...args),
  executeRemoteDrop: (...args: unknown[]) => dropMocks.executeRemoteDrop(...args),
}))

import { routeBrowserFileDrop } from './external-drop-router'

const SEL_KEY: Record<string, string> = {
  [COMPOSE_BAR_SELECTOR]: 'composeBar',
  [COMPOSE_DROP_SURFACE_SELECTOR]: 'composeDropSurface',
  '[data-terminal-id]': 'terminalId',
  '[data-terminal-container]': 'terminalContainer',
  '[data-file-tree-panel]': 'fileTreePanel',
  '[data-path]': 'path',
}

type Fake = {
  dataset: Record<string, string>
  _children: Fake[]
  _parent: Fake | null
  _events: Event[]
  closest: (sel: string) => Fake | null
  querySelector: (sel: string) => Fake | null
  contains: (other: Fake) => boolean
  dispatchEvent: (ev: Event) => boolean
  getBoundingClientRect: () => { left: number; right: number; top: number; bottom: number }
}

function matches(el: Fake, sel: string): boolean {
  const key = SEL_KEY[sel]
  return key ? Object.prototype.hasOwnProperty.call(el.dataset, key) : false
}

function node(dataset: Record<string, string>, children: Fake[] = []): Fake {
  const el: Fake = {
    dataset,
    _children: children,
    _parent: null,
    _events: [],
    closest(sel) {
      let cur: Fake | null = el
      while (cur) {
        if (matches(cur, sel)) return cur
        cur = cur._parent
      }
      return null
    },
    querySelector(sel) {
      const walk = (n: Fake): Fake | null => {
        for (const child of n._children) {
          if (matches(child, sel)) return child
          const inner = walk(child)
          if (inner) return inner
        }
        return null
      }
      return walk(el)
    },
    contains(other) {
      let cur: Fake | null = other
      while (cur) {
        if (cur === el) return true
        cur = cur._parent
      }
      return false
    },
    dispatchEvent(ev) {
      el._events.push(ev)
      return true
    },
    getBoundingClientRect() {
      return { left: 0, right: 10, top: 0, bottom: 10 }
    },
  }
  for (const child of children) child._parent = el
  return el
}

function asEl(el: Fake): HTMLElement {
  return el as unknown as HTMLElement
}

describe('compose surface image types', () => {
  const exts = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'heic', 'heif', 'svg']

  it('accepts the composer preview extensions and rejects pdf and text', () => {
    for (const ext of exts) {
      const path = `/tmp/shot.${ext}`
      expect(isComposeSurfaceImagePath(path), ext).toBe(true)
      expect(isComposePreviewImagePath(path), ext).toBe(true)
      expect(isComposeSurfaceImagePath(`/tmp/shot.${ext.toUpperCase()}`), ext).toBe(true)
    }
    expect(isComposeSurfaceImagePath('/tmp/notes.txt')).toBe(false)
    expect(isComposeSurfaceImagePath('/tmp/report.pdf')).toBe(false)
    expect(isComposePreviewImagePath('/tmp/report.pdf')).toBe(false)
    expect(isComposeSurfaceImagePath('/tmp/Screen Shot.png')).toBe(true)
  })

  it('pathsAreComposeSurfaceImages is all-or-nothing', () => {
    expect(pathsAreComposeSurfaceImages([])).toBe(false)
    expect(pathsAreComposeSurfaceImages(['/a.png', '/b.jpg'])).toBe(true)
    expect(pathsAreComposeSurfaceImages(['/a.png', '/b.txt'])).toBe(false)
    expect(pathsAreComposeSurfaceImages(['/a.pdf'])).toBe(false)
  })

  it('files follow the filename, or a known image MIME when there is no extension', () => {
    expect(isComposeSurfaceImageFile({ name: 'a.PNG', type: '' })).toBe(true)
    expect(isComposeSurfaceImageFile({ name: 'a.jpeg', type: 'application/octet-stream' })).toBe(true)
    expect(isComposeSurfaceImageFile({ name: 'clip', type: 'image/png' })).toBe(true)
    expect(isComposeSurfaceImageFile({ name: 'clip', type: 'image/svg+xml' })).toBe(true)
    expect(isComposeSurfaceImageFile({ name: 'notes.txt', type: 'image/png' })).toBe(false)
    expect(isComposeSurfaceImageFile({ name: 'a.pdf', type: 'application/pdf' })).toBe(false)
    expect(isComposeSurfaceImageFile({ name: 'a.tiff', type: 'image/tiff' })).toBe(false)
    expect(isComposeSurfaceImageFile({ name: '', type: 'text/plain' })).toBe(false)
    expect(filesAreComposeSurfaceImages([])).toBe(false)
    expect(
      filesAreComposeSurfaceImages([
        { name: 'a.png', type: 'image/png' },
        { name: 'b.txt', type: 'text/plain' },
      ]),
    ).toBe(false)
  })
})

describe('in-app file drag compose target', () => {
  function surfaceWithBar(kind: string, sessionId: string) {
    const row = node({})
    const bar = node({ composeBar: '', sessionId, workspacePath: '/ws' })
    const surface = node({ composeDropSurface: kind }, [row, bar])
    return { row, bar, surface }
  }

  it('inserts quoted image paths with no bracketed paste', () => {
    const payload = formatInAppComposeInsert(['/tmp/Screen Shot.png', '/tmp/a.txt'])
    expect(payload).toBe("'/tmp/Screen Shot.png' /tmp/a.txt ")
    expect(payload.includes(BRACKETED_PASTE_START)).toBe(false)
  })

  it('image on the thread list targets that column’s compose bar', () => {
    const { row, bar } = surfaceWithBar('thread', 'thread-1')
    expect(inAppComposeDropElement(asEl(row), ['/tmp/shot.png'])).toBe(bar)
  })

  it('non-image on the thread list does not target compose', () => {
    const { row } = surfaceWithBar('thread', 'thread-1')
    expect(inAppComposeDropElement(asEl(row), ['/tmp/notes.txt'])).toBeNull()
    expect(inAppComposeDropElement(asEl(row), ['/tmp/shot.png', '/tmp/notes.txt'])).toBeNull()
    expect(inAppComposeDropElement(asEl(row), ['/tmp/report.pdf'])).toBeNull()
  })

  it('any file dropped on the compose bar still targets that bar', () => {
    const { bar } = surfaceWithBar('thread', 'thread-1')
    expect(inAppComposeDropElement(asEl(bar), ['/tmp/notes.txt'])).toBe(bar)
  })

  it('an image on the terminal or files drawer is not retargeted to compose', () => {
    const term = node({ terminalId: 'pty-1' })
    const grid = node({}, [])
    grid._parent = term
    term._children = [grid]
    expect(inAppComposeDropElement(asEl(grid), ['/tmp/shot.png'])).toBeNull()

    const fileRow = node({ path: '/ws/shot.png' })
    const panel = node({ fileTreePanel: '', rootPath: '/ws' }, [fileRow])
    expect(inAppComposeDropElement(asEl(fileRow), ['/ws/shot.png'])).toBeNull()
    expect(panel.dataset.rootPath).toBe('/ws')
  })

  it('chat overlay image targets the chat bar, not the thread bar', () => {
    const thread = surfaceWithBar('thread', 'thread-1')
    const chat = surfaceWithBar('chat', 'chat-1')
    thread.surface._parent = null
    chat.surface._parent = null
    expect(inAppComposeDropElement(asEl(chat.row), ['/pic.webp'])).toBe(chat.bar)
    expect(inAppComposeDropElement(asEl(thread.row), ['/pic.gif'])).toBe(thread.bar)
  })
})

describe('routeBrowserFileDrop image surface', () => {
  beforeEach(() => {
    dropMocks.executeBrowserFileDrop.mockClear()
    dropMocks.executeBrowserFileDrop.mockResolvedValue('/host/shot.png ')
  })

  function docOf(el: Fake | null, panels: Fake[] = []): Document {
    return {
      elementFromPoint: () => el,
      querySelectorAll: (sel: string) =>
        sel === '[data-file-tree-panel]' ? panels : [],
    } as unknown as Document
  }

  it('uploads an image dropped on the thread list and inserts into that compose bar', async () => {
    const row = node({})
    const bar = node({ composeBar: '', sessionId: 'thread-1', workspacePath: '/ws' })
    const surface = node({ composeDropSurface: 'thread' }, [row, bar])
    const file = { name: 'shot.png', type: 'image/png' } as File
    await routeBrowserFileDrop([file], { x: 1, y: 1 }, docOf(row))
    expect(dropMocks.executeBrowserFileDrop).toHaveBeenCalledWith(
      [file],
      { kind: 'terminal' },
      { workspacePath: '/ws' },
      expect.any(Function),
    )
    expect(bar._events).toHaveLength(1)
    expect(bar._events[0].type).toBe('k2so:compose-insert')
    expect((bar._events[0] as CustomEvent).detail).toEqual({ data: '/host/shot.png ' })
    expect(surface.dataset.composeDropSurface).toBe('thread')
  })

  it('does not attach a non-image dropped on the thread list', async () => {
    const row = node({})
    const bar = node({ composeBar: '', sessionId: 'thread-1', workspacePath: '/ws' })
    node({ composeDropSurface: 'thread' }, [row, bar])
    const file = { name: 'notes.txt', type: 'text/plain' } as File
    await routeBrowserFileDrop([file], { x: 1, y: 1 }, docOf(row))
    expect(dropMocks.executeBrowserFileDrop).toHaveBeenCalledWith(
      [file],
      { kind: 'miss' },
      {},
    )
    expect(bar._events).toHaveLength(0)
  })

  it('keeps an image dropped on the files drawer on the folder path', async () => {
    const fileRow = node({ path: '/ws/shot.png', isDirectory: 'false' })
    const panel = node({ fileTreePanel: '', rootPath: '/ws' }, [fileRow])
    const file = { name: 'shot.png', type: 'image/png' } as File
    await routeBrowserFileDrop([file], { x: 1, y: 1 }, docOf(fileRow, [panel]))
    expect(dropMocks.executeBrowserFileDrop).toHaveBeenCalledWith(
      [file],
      { kind: 'folder', path: '/ws' },
      {},
    )
  })
})
