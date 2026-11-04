// @vitest-environment jsdom
//
// The tab around the canvas: routing, the cards a file gets instead of a board, the banners, the chip, and the leave
// guard. The frame is the widget's own suite (tests/board-file); here it's stubbed at its bundle fetch.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const web = vi.hoisted(() => ({ on: false }))
const features = vi.hoisted(() => ({ board: true }))
vi.mock('@/lib/daemon-cli', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: (...a: unknown[]) => daemonCliGet(...a),
  daemonCliPost: (...a: unknown[]) => daemonCliPost(...a),
}))
vi.mock('@/lib/is-web', async (importOriginal) => ({ ...(await importOriginal<typeof import('@/lib/is-web')>()), isWebClient: () => web.on }))
vi.mock('@/lib/zen/zen-widget-routes', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/zen/zen-widget-routes')>()),
  fetchZenWidgetBundle: vi.fn(async () => ({ html: '<!doctype html><title>x</title>', nonce: 'AAAAAAAAAAAAAAAAAAAAAA', hash: 'h' })),
}))
vi.mock('@/lib/zen/zen-lib-loader', () => ({ zenLib: { texts: vi.fn(async () => []) }, ZenLibError: class extends Error {} }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { primaryRoom } from '@/stores/room'
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { __resetServerScopesForTests, noteServerVersion } from '@/kessel/server-scope'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { BoardFilePane } from './BoardFilePane'
import { FileViewerPane } from './FileViewerPane'
import { createFakeBoardDaemon, httpErr, type FakeBoardDaemon, type FakeObject } from '@/lib/board-file/fake-board-daemon'
import { onlyBoardFileSessionForTests, resetBoardFileTabsForTests } from '@/lib/board-file/board-file-tabs'

const PATH = '/w/Plan.k5board'
const sticky = (id: string): FakeObject => ({ id, type: 'sticky', x: 1, y: 2, text: 't' })
let d: FakeBoardDaemon

function wire(daemon: FakeBoardDaemon): void {
  d = daemon
  daemonCliGet.mockImplementation((_s: unknown, route: string, params: Record<string, string | number | undefined>) => d.get(route, params))
  daemonCliPost.mockImplementation((_s: unknown, route: string, body: unknown) => d.post(route, body))
}
/** A note left by a K2 that went down, under this room's own server key (read once a pane has told us the key). */
const primaryConnectionKey = (): string => primaryRoom().scope.connectionKey
const noteLost = (edits: number): void => {
  const key = 'k2.boardFile.unsaved:'
  // The room's connection key is `local` for the primary room.
  localStorage.setItem(`${key}${primaryConnectionKey()}\n${PATH}`, JSON.stringify({ edits }))
}
const pane = (props: Partial<React.ComponentProps<typeof BoardFilePane>> = {}): ReturnType<typeof renderInPrimaryRoom> =>
  renderInPrimaryRoom(<BoardFilePane filePath={PATH} paneId="p" paneGroupId="pg" tabId="tab" {...props} />)
const session = onlyBoardFileSessionForTests

beforeEach(() => {
  web.on = false
  features.board = true
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  wire(createFakeBoardDaemon({ objects: [sticky('s-a')] }))
})
afterEach(() => {
  cleanup()
  resetBoardFileTabsForTests()
  __resetServerScopesForTests()
})

describe('routing', () => {
  it('a server without the feature gets the card, and no route is called', async () => {
    noteServerVersion(LOCAL_HOME_HOST, null, [])
    renderInPrimaryRoom(<FileViewerPane filePath={PATH} paneId="p" paneGroupId="pg" tabId="t" />)
    expect(await screen.findByText('This is a Board file. Update K2 to open it.')).toBeTruthy()
    expect(daemonCliGet).not.toHaveBeenCalled()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a server with the feature draws the board, and every door that mounts the viewer gets it', async () => {
    noteServerVersion(LOCAL_HOME_HOST, null, ['board-file-v1'])
    renderInPrimaryRoom(<FileViewerPane filePath={PATH} paneId="p" paneGroupId="pg" tabId="t" />)
    await waitFor(() => expect(screen.getByTestId('board-file-frame')).toBeTruthy())
    expect(daemonCliGet.mock.calls.map((c) => c[1])).not.toContain('fs/read-file')
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('preview off on the server (a tab restored early): the card, and no write', async () => {
    d.failNext('board/file/read', httpErr(404, 'not_enabled'))
    pane()
    expect(await screen.findByText('This is a Board file. Update K2 to open it.')).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('the web client gets the stub, and reads nothing', () => {
    web.on = true
    pane()
    expect(screen.getByText('Board needs the desktop app')).toBeTruthy()
    expect(daemonCliGet).not.toHaveBeenCalled()
  })

  it('a pane with no column (the dashboard) offers a tab, and reads nothing', () => {
    pane({ paneGroupId: undefined })
    expect(screen.getByText('Open in a tab')).toBeTruthy()
    expect(daemonCliGet).not.toHaveBeenCalled()
  })
})

describe('what a file gets instead of a board', () => {
  it('opening a board reads, and fires no write route', async () => {
    pane()
    await waitFor(() => expect(screen.getByTestId('board-file-frame')).toBeTruthy())
    expect(daemonCliGet.mock.calls.map((c) => c[1])).toContain('board/file/read')
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('empty: Start a board here is the person’s click', async () => {
    d.setEmpty(true)
    pane()
    const btn = await screen.findByTestId('board-file-start')
    expect(screen.getByText('This file is empty')).toBeTruthy()
    expect(daemonCliPost).not.toHaveBeenCalled()
    fireEvent.click(btn)
    await waitFor(() => expect(screen.getByTestId('board-file-frame')).toBeTruthy())
    expect(daemonCliPost.mock.calls.map((c) => c[1])).toEqual(['board/file/start'])
  })

  it('big: the numbers and Open anyway, then allowLarge', async () => {
    d.setLarge(64 * 1024 * 1024)
    pane()
    expect(await screen.findByText('This board is large (64 MiB)')).toBeTruthy()
    fireEvent.click(screen.getByTestId('board-file-open-anyway'))
    await waitFor(() => expect(screen.getByTestId('board-file-frame')).toBeTruthy())
    expect(daemonCliGet.mock.calls.some((c) => c[1] === 'board/file/read' && c[2].allowLarge === 1)).toBe(true)
  })

  it('newer: the numbers, never drawn', async () => {
    d.failNext('board/file/read', httpErr(422, 'min_reader_too_high', { needsReader: 3, ours: 1 }))
    pane()
    expect(await screen.findByText('Made by a newer K2 (needs reader 3, this is 1)')).toBeTruthy()
    expect(screen.queryByTestId('board-file-frame')).toBeNull()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('corrupt: reason, code, part and bad ids, and no way to write', async () => {
    d.failNext('board/file/read', httpErr(422, 'id_invalid', { at: 'board.json', badIds: ['b@d'], badIdsTotal: 7, badIdsTruncated: true }, 'an id isn’t valid'))
    pane()
    expect(await screen.findByText('K2 can’t read this board')).toBeTruthy()
    expect(screen.getByText('an id isn’t valid')).toBeTruthy()
    expect(screen.getByTestId('board-file-code').textContent).toBe('id_invalid (board.json)')
    expect(screen.getByTestId('board-file-bad-ids').textContent).toContain('7 bad ids: b@d…')
    expect(screen.queryByTestId('board-file-frame')).toBeNull()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })
})

describe('the banners and the chip', () => {
  async function open(): Promise<ReturnType<typeof renderInPrimaryRoom>> {
    const view = pane()
    await waitFor(() => expect(screen.getByTestId('board-file-frame')).toBeTruthy())
    return view
  }

  it('the file goes away: read only, edits kept, Save a copy works, and nothing recreates the file', async () => {
    await open()
    await act(async () => {
      session().submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
      d.failAll(httpErr(404, 'not_found'))
      await session().flush()
    })
    expect(screen.getByText(/This file was moved or deleted/)).toBeTruthy()
    await act(async () => {
      d.failAll(null)
      d.failNext('board/file/copy', httpErr(404, 'not_found'))
    })
    fireEvent.click(screen.getByTestId('board-file-save-copy'))
    const to = screen.getByTestId('board-file-copy-to') as HTMLInputElement
    expect(to.value).toBe('/w/Plan copy.k5board')
    fireEvent.change(to, { target: { value: '/elsewhere/Plan.k5board' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('board-file-copy-go'))
    })
    await waitFor(() => expect(d.copies).toHaveLength(1))
    expect(d.copies[0].to).toBe('/elsewhere/Plan.k5board')
    expect(d.copies[0].kind).toBe('rescue')
    expect(((d.copies[0].body.objects as FakeObject[])[0]).x).toBe(9)
    expect(d.writes.filter((w) => w.route === 'board/file/create' || w.route === 'board/file/start')).toEqual([])
  })

  it('Save a copy without a picture it can’t send: asks with the count, then goes', async () => {
    wire(createFakeBoardDaemon({ objects: [{ ...sticky('i-a'), type: 'image', src: 'k2blob:sha256-gone' }] }))
    await open()
    await act(async () => {
      d.failAll(httpErr(404, 'not_found'))
      await session().pollInfo()
    })
    await act(async () => d.failAll(null))
    fireEvent.click(screen.getByTestId('board-file-save-copy'))
    await act(async () => {
      fireEvent.click(screen.getByTestId('board-file-copy-go'))
    })
    expect((await screen.findByTestId('board-file-copy-missing')).textContent).toBe('1 picture is gone.')
    expect(d.copies).toEqual([])
    await act(async () => {
      fireEvent.click(screen.getByTestId('board-file-copy-without'))
    })
    await waitFor(() => expect(d.copies).toHaveLength(1))
    expect(d.copies[0].body.dropMissing).toBe(true)
  })

  it('a copy that can’t land says why and keeps the form', async () => {
    await open()
    await act(async () => {
      session().submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
      d.failNext('board/file/save', httpErr(503, 'disk_full'), 100)
      await session().flush()
    })
    expect(screen.getByText(/the disk is full/)).toBeTruthy()
    d.failNext('board/file/copy', httpErr(503, 'disk_full', {}, 'that disk is full too'))
    fireEvent.click(screen.getByTestId('board-file-save-copy'))
    await act(async () => {
      fireEvent.click(screen.getByTestId('board-file-copy-go'))
    })
    expect((await screen.findByTestId('board-file-copy-error')).textContent).toBe('that disk is full too')
    expect(session().pendingCount()).toBe(1)
  })

  async function held(): Promise<void> {
    await open()
    await act(async () => {
      d.outsideSet('s-a', { text: 'theirs' })
      session().submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { text: 'mine' } }] })
      await session().flush()
    })
  }

  it('a held edit: the chip says it isn’t in the file, the list shows both sides, and Use theirs clears it', async () => {
    await held()
    const chip = await screen.findByTestId('board-file-held-chip')
    expect(chip.textContent).toContain('Not in the file until you choose')
    fireEvent.click(chip)
    const fields = screen.getByTestId('board-file-held-fields')
    expect(fields.textContent).toContain('text')
    expect(fields.textContent).toContain('mine')
    expect(fields.textContent).toContain('theirs')
    await act(async () => {
      fireEvent.click(screen.getByTestId('board-file-use-theirs'))
    })
    expect(screen.queryByTestId('board-file-held-chip')).toBeNull()
    expect(d.objects()[0].text).toBe('theirs')
  })

  it('Use mine sends it', async () => {
    await held()
    fireEvent.click(await screen.findByTestId('board-file-held-chip'))
    await act(async () => {
      fireEvent.click(screen.getByTestId('board-file-use-mine'))
      await session().flush()
    })
    await waitFor(() => expect(d.objects()[0].text).toBe('mine'))
  })

  it('the tab’s Close asks about a held edit first', async () => {
    const confirm = vi.fn(async () => false)
    useConfirmDialogStore.setState({ confirm } as never)
    await open()
    await act(async () => {
      session().submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
      d.failAll(httpErr(404, 'not_found'))
      await session().flush()
    })
    await act(async () => {
      fireEvent.click(screen.getByText('Close'))
    })
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(session().pendingCount()).toBe(1)
  })

  it('K2 going down with an edit is said on reopen', async () => {
    noteLost(2)
    await open()
    expect(screen.getByText(/K2 closed with 2 edits to this board that hadn’t been saved/)).toBeTruthy()
    fireEvent.click(screen.getByTestId('board-file-lost-ok'))
    expect(screen.queryByText(/K2 closed with/)).toBeNull()
  })

  it('a dangling picture says the count, and clearing it is the person’s click', async () => {
    wire(createFakeBoardDaemon({ objects: [sticky('s-a')], missing: ['sha256-a', 'sha256-b'] }))
    await open()
    expect(screen.getByText('2 pictures are missing from this file.')).toBeTruthy()
    expect(screen.queryByTestId('board-file-clear-missing')).toBeNull()
    await act(async () => {
      d.setSaveRefusal({ code: 'blob_missing' })
      session().submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
      await session().flush()
    })
    expect(screen.getByTestId('board-file-clear-missing').textContent).toBe('Clear 2 and keep saving')
    expect(session().pendingCount()).toBe(1)
  })
})
