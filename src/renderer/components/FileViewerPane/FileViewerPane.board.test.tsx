// @vitest-environment jsdom
//
// A `.k5board` is a zip that only the format's codec may touch. Every door that
// opens a file mounts FileViewerPane, so the one routing point is what stops
// the text read, poll, Save, autosave-on-leave and Live view.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
    daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
  }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { FileViewerPane, type FileViewerHandle } from './FileViewerPane'

const CARD = 'This is a Board file. Update K2 to open it.'
const allCalls = (): string[] => [...daemonCliGet.mock.calls, ...daemonCliPost.mock.calls].map(([, route]) => String(route))

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    throw new Error(`a Board file must not call ${route}`)
  })
  daemonCliPost.mockImplementation(async (_scope: unknown, route: string) => {
    throw new Error(`a Board file must not call ${route}`)
  })
})
afterEach(() => cleanup())

describe('a .k5board in the file viewer', () => {
  for (const path of ['/w/Plan.k5board', '/w/PLAN.K5BOARD', '/w/Makefile.k5board', '/w/.env.k5board']) {
    it(`${path}: shows the card, and reads and writes nothing, even on Cmd+S and unmount`, async () => {
      const ref = React.createRef<FileViewerHandle | null>()
      const view = renderInPrimaryRoom(
        <FileViewerPane filePath={path} paneId="p" paneGroupId="pg" tabId="t" commandRef={ref} />,
      )
      expect(screen.getByText(CARD)).toBeTruthy()
      fireEvent.keyDown(screen.getByText(CARD), { key: 's', metaKey: true })
      expect(ref.current).toBeNull()
      view.unmount()
      await new Promise((r) => setTimeout(r, 0))
      expect(allCalls()).toEqual([])
    })
  }

  it('leaves a text file on the text path', async () => {
    daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
      if (route === 'fs/read-file') return { content: 'hello' }
      return {}
    })
    renderInPrimaryRoom(<FileViewerPane filePath="/w/a.txt" paneId="p" paneGroupId="pg" tabId="t" />)
    expect(screen.queryByText(CARD)).toBeNull()
    await new Promise((r) => setTimeout(r, 0))
    expect(allCalls()).toContain('fs/read-file')
  })
})
