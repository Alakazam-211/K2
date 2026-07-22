// Pure hit-test / classify helpers for the single external-drop router.
// DOM-dependent route execution is covered by handle-remote-drop tests +
// planLocalExternalDrop; this file pins the routing priority table.

import { describe, it, expect } from 'vitest'
import {
  parentDir,
  resolveFileTreeFolder,
  classifyExternalDrop,
  buildTerminalDropPayload,
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
