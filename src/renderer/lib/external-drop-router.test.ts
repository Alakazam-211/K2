// Pure hit-test / classify helpers for the single external-drop router.
// DOM-dependent route execution is covered by handle-remote-drop tests +
// planLocalExternalDrop; this file pins the routing priority table.

import { describe, it, expect } from 'vitest'
import {
  parentDir,
  resolveFileTreeFolder,
  classifyExternalDrop,
  buildTerminalDropPayload,
  pointInRect,
  panelOwnsFolderPath,
  findFileTreePanelAt,
  notifyFileTreeRefresh,
  FILE_TREE_EXTERNAL_DROP_EVENT,
} from './external-drop-router'
import { BRACKETED_PASTE_START, BRACKETED_PASTE_END } from './file-drag'

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

  it('prefers terminal over file-tree folder', () => {
    const target = classifyExternalDrop({
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
      terminal: null,
      fileTreeFolder: '/ws/inbox',
    })
    expect(target).toEqual({ kind: 'folder', path: '/ws/inbox' })
  })

  it('routes to miss when neither terminal nor files panel is hit', () => {
    const target = classifyExternalDrop({
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

