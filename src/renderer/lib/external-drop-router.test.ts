// Pure hit-test / classify helpers for the single external-drop router.
// DOM-dependent route execution is covered by handle-remote-drop tests +
// planLocalExternalDrop; this file pins the routing priority table.
//
// mountExternalDropRouter is tested with a webview-scoped listen mock so
// multi-window never regresses to process-global event.listen Any, and so
// we never use getCurrentWindow().listen (Window target misses drag-*).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const webviewListenMock = vi.fn()
const webviewUnlistenMock = vi.fn()
const isWebClientMock = vi.fn(() => false)

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    // Return value of the spy is the listen Promise (so tests can delay resolve).
    listen: (...args: unknown[]) => webviewListenMock(...args),
  }),
}))

vi.mock('./is-web', () => ({
  isWebClient: () => isWebClientMock(),
}))

import {
  parentDir,
  resolveFileTreeFolder,
  classifyExternalDrop,
  buildTerminalDropPayload,
  buildComposeDropPayload,
  pointInRect,
  panelOwnsFolderPath,
  findFileTreePanelAt,
  notifyFileTreeRefresh,
  filesFromDataTransfer,
  hitTestExternalDrop,
  routeExternalDrop,
  mountExternalDropRouter,
  FILE_TREE_EXTERNAL_DROP_EVENT,
} from './external-drop-router'
import { useConnectHostStore } from '@/stores/connect-host'
import { COMPOSE_BAR_SELECTOR, COMPOSE_DROP_SURFACE_SELECTOR } from './compose-surface-drop'
import { BRACKETED_PASTE_START, BRACKETED_PASTE_END } from './file-drag'

import { primaryScope } from '@/kessel/server-scope'
describe('parentDir', () => {
  it('returns the parent of a nested path', () => {
    expect(parentDir('/ws/docs/a.txt')).toBe('/ws/docs')
  })

  it('returns / for a top-level path', () => {
    expect(parentDir('/a.txt')).toBe('/')
  })
})

describe('resolveFileTreeFolder', () => {
  it('returns null when outside the panel', () => {
    expect(
      resolveFileTreeFolder({
        inPanel: false,
        rootPath: '/ws',
        rowPath: '/ws/docs',
        rowIsDirectory: true,
      }),
    ).toBeNull()
  })

  it('uses the row path when the row is a directory', () => {
    expect(
      resolveFileTreeFolder({
        inPanel: true,
        rootPath: '/ws',
        rowPath: '/ws/docs',
        rowIsDirectory: true,
      }),
    ).toBe('/ws/docs')
  })

  it('uses the parent of a file row', () => {
    expect(
      resolveFileTreeFolder({
        inPanel: true,
        rootPath: '/ws',
        rowPath: '/ws/docs/report.pdf',
        rowIsDirectory: false,
      }),
    ).toBe('/ws/docs')
  })

  it('falls back to the tree root when no row is under the cursor', () => {
    expect(
      resolveFileTreeFolder({
        inPanel: true,
        rootPath: '/ws',
        rowPath: null,
        rowIsDirectory: false,
      }),
    ).toBe('/ws')
  })
})

describe('classifyExternalDrop', () => {
  const fakeEl = { tagName: 'DIV' } as unknown as HTMLElement

  it('prefers compose over terminal and file-tree', () => {
    const target = classifyExternalDrop({
      compose: {
        sessionId: 'sess-1',
        workspacePath: '/ws',
        element: fakeEl,
      },
      terminal: {
        terminalId: 't1',
        terminalKind: 'v2',
        workspacePath: '/ws',
        element: fakeEl,
      },
      fileTreeFolder: '/ws/docs',
    })
    expect(target).toMatchObject({
      kind: 'compose',
      sessionId: 'sess-1',
      workspacePath: '/ws',
    })
  })

  it('prefers terminal over file-tree folder', () => {
    const target = classifyExternalDrop({
      compose: null,
      terminal: {
        terminalId: 't1',
        terminalKind: 'v2',
        workspacePath: '/ws',
        element: fakeEl,
      },
      fileTreeFolder: '/ws/docs',
    })
    expect(target).toMatchObject({
      kind: 'terminal',
      terminalId: 't1',
      workspacePath: '/ws',
    })
  })

  it('routes to folder when no terminal is hit', () => {
    const target = classifyExternalDrop({
      compose: null,
      terminal: null,
      fileTreeFolder: '/ws/inbox',
    })
    expect(target).toEqual({ kind: 'folder', path: '/ws/inbox' })
  })

  it('routes to miss when neither terminal nor files panel is hit', () => {
    const target = classifyExternalDrop({
      compose: null,
      terminal: null,
      fileTreeFolder: null,
    })
    expect(target).toEqual({ kind: 'miss' })
  })
})

describe('buildTerminalDropPayload', () => {
  it('joins plain paths with a trailing space', () => {
    expect(buildTerminalDropPayload(['/tmp/a.txt', '/tmp/b.txt'])).toBe(
      '/tmp/a.txt /tmp/b.txt ',
    )
  })

  it('wraps image drops in bracketed paste', () => {
    const payload = buildTerminalDropPayload(['/tmp/shot.png'])
    expect(payload.startsWith(BRACKETED_PASTE_START)).toBe(true)
    expect(payload.endsWith(BRACKETED_PASTE_END)).toBe(true)
    expect(payload).toContain('/tmp/shot.png')
  })
})

describe('buildComposeDropPayload', () => {
  it('joins paths with a trailing space and no bracketed paste', () => {
    const payload = buildComposeDropPayload(['/tmp/a.txt', '/tmp/shot.png'])
    expect(payload.endsWith(' ')).toBe(true)
    expect(payload).toContain('/tmp/a.txt')
    expect(payload).toContain('/tmp/shot.png')
    expect(payload.startsWith(BRACKETED_PASTE_START)).toBe(false)
  })
})

describe('pointInRect / panelOwnsFolderPath', () => {
  it('pointInRect is inclusive on edges', () => {
    const rect = { left: 10, right: 20, top: 5, bottom: 15 }
    expect(pointInRect({ x: 10, y: 5 }, rect)).toBe(true)
    expect(pointInRect({ x: 20, y: 15 }, rect)).toBe(true)
    expect(pointInRect({ x: 9, y: 10 }, rect)).toBe(false)
  })

  it('panelOwnsFolderPath matches root and descendants only', () => {
    expect(panelOwnsFolderPath('/ws', '/ws')).toBe(true)
    expect(panelOwnsFolderPath('/ws', '/ws/docs')).toBe(true)
    expect(panelOwnsFolderPath('/ws', '/ws-other')).toBe(false)
    expect(panelOwnsFolderPath('/ws/a', '/ws/b')).toBe(false)
  })
})

describe('findFileTreePanelAt — multi-panel', () => {
  function fakePanel(
    rootPath: string,
    rect: { left: number; right: number; top: number; bottom: number },
  ) {
    return {
      dataset: { rootPath, fileTreePanel: 'true' },
      getBoundingClientRect: () => rect,
      closest: (sel: string) =>
        sel === '[data-file-tree-panel]' ? fakePanel(rootPath, rect) : null,
      contains: () => false,
    } as unknown as HTMLElement
  }

  it('prefers the panel containing the element under the point', () => {
    const left = fakePanel('/ws-left', { left: 0, right: 100, top: 0, bottom: 200 })
    const right = fakePanel('/ws-right', { left: 200, right: 300, top: 0, bottom: 200 })
    const under = {
      closest: (sel: string) => (sel === '[data-file-tree-panel]' ? right : null),
    } as unknown as HTMLElement
    const doc = {
      querySelectorAll: () => [left, right],
    } as unknown as Document

    const hit = findFileTreePanelAt({ x: 10, y: 10 }, under, doc)
    expect(hit).toBe(right)
  })

  it('falls back to rect hit-test when closest is null (empty padding)', () => {
    const left = fakePanel('/ws-left', { left: 0, right: 100, top: 0, bottom: 200 })
    const right = fakePanel('/ws-right', { left: 200, right: 300, top: 0, bottom: 200 })
    const doc = {
      querySelectorAll: () => [left, right],
    } as unknown as Document

    expect(findFileTreePanelAt({ x: 250, y: 50 }, null, doc)).toBe(right)
    expect(findFileTreePanelAt({ x: 50, y: 50 }, null, doc)).toBe(left)
    expect(findFileTreePanelAt({ x: 150, y: 50 }, null, doc)).toBeNull()
  })
})

describe('notifyFileTreeRefresh — multi-panel', () => {
  it('dispatches only on panels that own the dest folder', () => {
    const leftEvents: Event[] = []
    const rightEvents: Event[] = []
    const left = {
      dataset: { rootPath: '/ws-left' },
      dispatchEvent: (e: Event) => {
        leftEvents.push(e)
        return true
      },
    }
    const right = {
      dataset: { rootPath: '/ws-right' },
      dispatchEvent: (e: Event) => {
        rightEvents.push(e)
        return true
      },
    }
    const doc = {
      querySelectorAll: () => [left, right],
    } as unknown as Document

    notifyFileTreeRefresh('/ws-right/inbox', doc)
    expect(leftEvents).toHaveLength(0)
    expect(rightEvents).toHaveLength(1)
    expect((rightEvents[0] as CustomEvent).type).toBe(FILE_TREE_EXTERNAL_DROP_EVENT)
    expect((rightEvents[0] as CustomEvent).detail).toEqual({ path: '/ws-right/inbox' })
  })

  it('falls back to all panels when none declare a matching rootPath', () => {
    const events: string[] = []
    const a = {
      dataset: {},
      dispatchEvent: (e: Event) => {
        events.push((e as CustomEvent).type)
        return true
      },
    }
    const b = {
      dataset: {},
      dispatchEvent: (e: Event) => {
        events.push((e as CustomEvent).type)
        return true
      },
    }
    const doc = { querySelectorAll: () => [a, b] } as unknown as Document
    notifyFileTreeRefresh('/any', doc)
    expect(events).toEqual([FILE_TREE_EXTERNAL_DROP_EVENT, FILE_TREE_EXTERNAL_DROP_EVENT])
  })
})


// ── Hosted web File list helper ─────────────────────────────────────────

describe('filesFromDataTransfer', () => {
  it('returns empty for null / empty list', () => {
    expect(filesFromDataTransfer(null)).toEqual([])
    const dt = { files: { length: 0, item: () => null } } as unknown as DataTransfer
    expect(filesFromDataTransfer(dt)).toEqual([])
  })

  it('collects File items in order', () => {
    const a = new File([new Uint8Array([1])], 'a.txt', { type: 'text/plain' })
    const b = new File([new Uint8Array([2])], 'b.txt', { type: 'text/plain' })
    const list = {
      length: 2,
      item: (i: number) => (i === 0 ? a : i === 1 ? b : null),
    }
    const dt = { files: list } as unknown as DataTransfer
    expect(filesFromDataTransfer(dt).map((f) => f.name)).toEqual(['a.txt', 'b.txt'])
  })
})

// ── Webview-scoped Tauri drag-drop subscription ───────────────────────

describe('mountExternalDropRouter (desktop)', () => {
  beforeEach(() => {
    isWebClientMock.mockReturnValue(false)
    webviewListenMock.mockReset()
    webviewUnlistenMock.mockReset()
    webviewListenMock.mockResolvedValue(webviewUnlistenMock)
  })

  afterEach(() => {
    isWebClientMock.mockReturnValue(false)
  })

  it('subscribes via getCurrentWebview().listen, not process-global or Window', async () => {
    const unmount = mountExternalDropRouter()
    // Dynamic import of @tauri-apps/api/webview resolves on next microtask.
    await vi.waitFor(() => {
      expect(webviewListenMock).toHaveBeenCalled()
    })
    expect(webviewListenMock).toHaveBeenCalledWith(
      'tauri://drag-drop',
      expect.any(Function),
    )
    unmount()
    expect(webviewUnlistenMock).toHaveBeenCalled()
  })

  it('teardown before listen resolves does not leave a live handler', async () => {
    let resolveListen: (fn: () => void) => void = () => {}
    const delayedUnlisten = vi.fn()
    webviewListenMock.mockImplementationOnce(
      () =>
        new Promise<() => void>((resolve) => {
          resolveListen = resolve
        }),
    )

    const unmount = mountExternalDropRouter()
    await vi.waitFor(() => {
      expect(webviewListenMock).toHaveBeenCalled()
    })
    // Unmount while listen() is still pending.
    unmount()
    resolveListen(delayedUnlisten)
    // track() runs on the next microtask after listen resolves.
    await vi.waitFor(() => {
      expect(delayedUnlisten).toHaveBeenCalled()
    })
  })

  it('web client path does not touch webview-scoped Tauri listen', () => {
    isWebClientMock.mockReturnValue(true)
    // Minimal window stub for the HTML5 drag/drop branch (vitest env is node).
    const addEventListener = vi.fn()
    const removeEventListener = vi.fn()
    const prev = (globalThis as { window?: unknown }).window
    ;(globalThis as { window: unknown }).window = {
      addEventListener,
      removeEventListener,
    }
    try {
      const unmount = mountExternalDropRouter()
      expect(webviewListenMock).not.toHaveBeenCalled()
      expect(addEventListener).toHaveBeenCalledWith('dragover', expect.any(Function))
      expect(addEventListener).toHaveBeenCalledWith('drop', expect.any(Function))
      unmount()
      expect(removeEventListener).toHaveBeenCalledWith('dragover', expect.any(Function))
      expect(removeEventListener).toHaveBeenCalledWith('drop', expect.any(Function))
    } finally {
      if (prev === undefined) {
        delete (globalThis as { window?: unknown }).window
      } else {
        ;(globalThis as { window: unknown }).window = prev
      }
    }
  })
})

// ── Thread / chat surface image drops ─────────────────────────────────

const SURFACE_SEL_KEY: Record<string, string> = {
  [COMPOSE_BAR_SELECTOR]: 'composeBar',
  [COMPOSE_DROP_SURFACE_SELECTOR]: 'composeDropSurface',
  '[data-terminal-id]': 'terminalId',
  '[data-terminal-container]': 'terminalContainer',
  '[data-file-tree-panel]': 'fileTreePanel',
  '[data-path]': 'path',
}

type SurfaceFake = {
  dataset: Record<string, string>
  _children: SurfaceFake[]
  _parent: SurfaceFake | null
  _events: Event[]
  closest: (sel: string) => SurfaceFake | null
  querySelector: (sel: string) => SurfaceFake | null
  contains: (other: SurfaceFake) => boolean
  dispatchEvent: (ev: Event) => boolean
  getBoundingClientRect: () => { left: number; right: number; top: number; bottom: number }
}

function surfaceMatches(el: SurfaceFake, sel: string): boolean {
  const key = SURFACE_SEL_KEY[sel]
  return key ? Object.prototype.hasOwnProperty.call(el.dataset, key) : false
}

function surfaceNode(dataset: Record<string, string>, children: SurfaceFake[] = []): SurfaceFake {
  const el: SurfaceFake = {
    dataset,
    _children: children,
    _parent: null,
    _events: [],
    closest(sel) {
      let cur: SurfaceFake | null = el
      while (cur) {
        if (surfaceMatches(cur, sel)) return cur
        cur = cur._parent
      }
      return null
    },
    querySelector(sel) {
      const walk = (n: SurfaceFake): SurfaceFake | null => {
        for (const child of n._children) {
          if (surfaceMatches(child, sel)) return child
          const found = walk(child)
          if (found) return found
        }
        return null
      }
      return walk(el)
    },
    contains(other) {
      let cur: SurfaceFake | null = other
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
      return { left: 0, right: 20, top: 0, bottom: 20 }
    },
  }
  for (const child of children) child._parent = el
  return el
}

function surfaceDoc(el: SurfaceFake | null, panels: SurfaceFake[] = []): Document {
  return {
    elementFromPoint: () => el,
    querySelectorAll: (sel: string) => (sel === '[data-file-tree-panel]' ? panels : []),
  } as unknown as Document
}

describe('hitTestExternalDrop — thread and chat image surfaces', () => {
  function threadColumn() {
    const row = surfaceNode({})
    const bar = surfaceNode({ composeBar: '', sessionId: 'thread-1', workspacePath: '/ws' })
    const surface = surfaceNode({ composeDropSurface: 'thread' }, [row, bar])
    return { row, bar, surface }
  }

  it('routes an image on the thread list to that column’s compose bar', () => {
    const { row, bar } = threadColumn()
    const target = hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(row), {
      composeSurfaceImages: true,
    })
    expect(target).toMatchObject({
      kind: 'compose',
      sessionId: 'thread-1',
      workspacePath: '/ws',
    })
    if (target.kind === 'compose') expect(target.element).toBe(bar)
  })

  it('leaves a non-image on the thread list as a miss', () => {
    const { row } = threadColumn()
    expect(
      hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(row), { composeSurfaceImages: false }),
    ).toEqual({ kind: 'miss' })
  })

  it('still routes any file dropped on the compose bar itself', () => {
    const { bar } = threadColumn()
    const target = hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(bar), {
      composeSurfaceImages: false,
    })
    expect(target).toMatchObject({ kind: 'compose', sessionId: 'thread-1' })
  })

  it('prefers the surface compose bar over a terminal ancestor for images only', () => {
    const { row, bar, surface } = threadColumn()
    const term = surfaceNode(
      { terminalId: 'pty-1', terminalKind: 'v2', workspacePath: '/ws', terminalContainer: '' },
      [surface],
    )
    const image = hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(row), {
      composeSurfaceImages: true,
    })
    expect(image).toMatchObject({ kind: 'compose', sessionId: 'thread-1' })
    if (image.kind === 'compose') expect(image.element).toBe(bar)

    const text = hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(row), {
      composeSurfaceImages: false,
    })
    expect(text).toMatchObject({ kind: 'terminal', terminalId: 'pty-1' })
    if (text.kind === 'terminal') expect(text.element).toBe(term)
  })

  it('does not steal an image drop from the terminal grid or the files drawer', () => {
    const grid = surfaceNode({})
    const term = surfaceNode(
      { terminalId: 'pty-1', terminalKind: 'v2', workspacePath: '/ws' },
      [grid],
    )
    const onGrid = hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(grid), {
      composeSurfaceImages: true,
    })
    expect(onGrid).toMatchObject({ kind: 'terminal', terminalId: 'pty-1' })
    if (onGrid.kind === 'terminal') expect(onGrid.element).toBe(term)

    const fileRow = surfaceNode({ path: '/ws/shot.png', isDirectory: 'false' })
    const panel = surfaceNode({ fileTreePanel: '', rootPath: '/ws' }, [fileRow])
    const onFiles = hitTestExternalDrop({ x: 4, y: 4 }, surfaceDoc(fileRow, [panel]), {
      composeSurfaceImages: true,
    })
    expect(onFiles).toEqual({ kind: 'folder', path: '/ws' })
  })

  it('routes a chat-overlay image to the chat bar, not the thread bar', () => {
    const thread = threadColumn()
    const chatRow = surfaceNode({})
    const chatBar = surfaceNode({ composeBar: '', sessionId: 'chat-1', workspacePath: '/ws' })
    surfaceNode({ composeDropSurface: 'chat' }, [chatRow, chatBar])
    const target = hitTestExternalDrop({ x: 2, y: 2 }, surfaceDoc(chatRow), {
      composeSurfaceImages: true,
    })
    expect(target).toMatchObject({ kind: 'compose', sessionId: 'chat-1' })
    if (target.kind === 'compose') expect(target.element).toBe(chatBar)
    expect(thread.bar._events).toHaveLength(0)
  })

  it('does not claim a surface that has no compose bar', () => {
    const row = surfaceNode({})
    surfaceNode({ composeDropSurface: 'thread' }, [row])
    expect(
      hitTestExternalDrop({ x: 1, y: 1 }, surfaceDoc(row), { composeSurfaceImages: true }),
    ).toEqual({ kind: 'miss' })
  })
})

describe('routeExternalDrop — thread surface images', () => {
  beforeEach(() => {
    useConnectHostStore.setState({ activeHost: 'local' })
  })

  it('inserts a local image path into the thread compose draft', async () => {
    const row = surfaceNode({})
    const bar = surfaceNode({ composeBar: '', sessionId: 'thread-1', workspacePath: '/ws' })
    surfaceNode({ composeDropSurface: 'thread' }, [row, bar])
    await routeExternalDrop(primaryScope(), ['/tmp/Screen Shot.png'], { x: 3, y: 3 }, surfaceDoc(row))
    expect(bar._events).toHaveLength(1)
    const ev = bar._events[0] as CustomEvent<{ data: string }>
    expect(ev.type).toBe('k2so:compose-insert')
    expect(ev.detail.data).toBe("'/tmp/Screen Shot.png' ")
    expect(ev.detail.data.startsWith('\u001b')).toBe(false)
  })

  it('does not insert a non-image dropped on the thread list', async () => {
    const row = surfaceNode({})
    const bar = surfaceNode({ composeBar: '', sessionId: 'thread-1', workspacePath: '/ws' })
    surfaceNode({ composeDropSurface: 'thread' }, [row, bar])
    await routeExternalDrop(primaryScope(), ['/tmp/notes.txt'], { x: 3, y: 3 }, surfaceDoc(row))
    expect(bar._events).toHaveLength(0)
  })

  it('still pastes an image dropped on the terminal grid into the PTY', async () => {
    const grid = surfaceNode({})
    const term = surfaceNode(
      { terminalId: 'pty-1', terminalKind: 'v2', workspacePath: '/ws' },
      [grid],
    )
    await routeExternalDrop(primaryScope(), ['/tmp/shot.png'], { x: 3, y: 3 }, surfaceDoc(grid))
    expect(term._events).toHaveLength(1)
    expect(term._events[0].type).toBe('k2so:terminal-write')
    expect((term._events[0] as CustomEvent<{ data: string }>).detail.data).toContain('/tmp/shot.png')
  })
})
