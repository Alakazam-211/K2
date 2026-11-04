// @vitest-environment jsdom
//
// K2 Cores' conditions on held edits, in the doors around a session: closing and quitting ask first, an update doesn't
// discard one silently, a pane going away doesn't lose one, and K2 going down without asking is said on reopen.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.hoisted(() => vi.fn(async (..._a: unknown[]) => undefined))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))

import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import {
  acquireBoardFileSession,
  askBeforeBoardFileLoss,
  boardFileSessionFor,
  releaseBoardFileSession,
  resetBoardFileTabsForTests,
  unsavedBoardFiles,
  unsavedBoardSessionsInTab,
} from './board-file-tabs'
import { createFakeBoardDaemon, httpErr, type FakeBoardDaemon, type FakeObject } from './fake-board-daemon'

const PATH = '/w/Plan.k5board'
const KEY = 'local'
const sticky = (id: string): FakeObject => ({ id, type: 'sticky', x: 1, y: 2, text: 't' })
const flushMicro = async (): Promise<void> => {
  for (let i = 0; i < 40; i++) await Promise.resolve()
}

let d: FakeBoardDaemon
const answers: boolean[] = []
const asked: { title: string; message: string; confirmLabel?: string }[] = []

function acquire(path = PATH): ReturnType<typeof acquireBoardFileSession> {
  return acquireBoardFileSession({ connectionKey: KEY, path, api: d, visible: () => true })
}
/** A session with one edit the file refused: held, in memory only. */
async function withHeldEdit(): Promise<ReturnType<typeof acquireBoardFileSession>> {
  const s = acquire()
  await s.start()
  d.outsideSet('s-a', { text: 'theirs' })
  s.submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { text: 'mine' } }] })
  await s.flush()
  await flushMicro()
  expect(s.heldCount()).toBe(1)
  return s
}

beforeEach(() => {
  localStorage.clear()
  invoke.mockClear()
  d = createFakeBoardDaemon({ objects: [sticky('s-a')] })
  answers.length = 0
  asked.length = 0
  useConfirmDialogStore.setState({
    confirm: async (o: { title: string; message: string; confirmLabel?: string }) => {
      asked.push(o)
      return answers.shift() ?? false
    },
  } as never)
})
afterEach(() => resetBoardFileTabsForTests())

describe('asking first', () => {
  it('nothing unsaved: no question, and the door opens', async () => {
    const s = acquire()
    await s.start()
    expect(await askBeforeBoardFileLoss('quit')).toBe(true)
    expect(asked).toEqual([])
  })

  it('queued edits flush first, so a clean save asks nothing', async () => {
    const s = acquire()
    await s.start()
    s.submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
    expect(await askBeforeBoardFileLoss('close')).toBe(true)
    expect(asked).toEqual([])
    expect(d.objects()[0].x).toBe(9)
  })

  for (const what of ['close', 'quit', 'update'] as const) {
    it(`${what} with a held edit asks; No stays and keeps it`, async () => {
      const s = await withHeldEdit()
      answers.push(false)
      expect(await askBeforeBoardFileLoss(what)).toBe(false)
      expect(asked).toHaveLength(1)
      expect(asked[0].title).toBe('1 edit isn’t saved')
      expect(asked[0].message).toContain('Plan.k5board')
      expect(s.heldCount()).toBe(1)
      expect(s.view().held).toHaveLength(1)
    })
  }

  it('Yes lets the edit go at the person’s say-so, and no note is left', async () => {
    const s = await withHeldEdit()
    answers.push(true)
    expect(await askBeforeBoardFileLoss('quit')).toBe(true)
    expect(s.heldCount()).toBe(0)
    expect(d.objects()[0].text).toBe('theirs')
    expect(Object.keys(localStorage).filter((k) => k.startsWith('k2.boardFile'))).toEqual([])
  })

  it('a disk-full queue also asks: those edits are only here', async () => {
    const s = acquire()
    await s.start()
    d.failNext('board/file/save', httpErr(503, 'disk_full'), 100)
    s.submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
    answers.push(false)
    expect(await askBeforeBoardFileLoss('update')).toBe(false)
    expect(asked[0].title).toBe('1 edit isn’t saved')
    expect(s.pendingCount()).toBe(1)
  })

  it('a tab’s close asks only about that tab’s files', async () => {
    await withHeldEdit()
    const other = acquire('/w/Other.k5board')
    await other.start()
    expect(await askBeforeBoardFileLoss('close', [other])).toBe(true)
    expect(asked).toEqual([])
  })

  it('finds a tab’s unsaved sessions by its file viewers, and no others', async () => {
    const s = await withHeldEdit()
    const tab = (paths: string[]): never => ({ paneGroups: new Map([['pg', { items: paths.map((filePath) => ({ type: 'file-viewer', data: { filePath } })) }]]) }) as never
    expect(unsavedBoardSessionsInTab(tab([PATH]), KEY)).toEqual([s])
    expect(unsavedBoardSessionsInTab(tab(['/w/a.txt', '/w/Clean.k5board']), KEY)).toEqual([])
    expect(unsavedBoardSessionsInTab(tab([PATH]), 'another-server')).toEqual([])
  })
})

describe('a pane going away', () => {
  it('nothing unsaved: the session ends', async () => {
    const s = acquire()
    await s.start()
    await releaseBoardFileSession(KEY, PATH, { discard: false })
    expect(boardFileSessionFor(KEY, PATH)).toBeNull()
    expect(s.pendingCount()).toBe(0)
  })

  it('queued edits flush on the way out, to the path the pane was mounted for', async () => {
    const s = acquire()
    await s.start()
    s.submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
    await releaseBoardFileSession(KEY, PATH, { discard: false })
    expect(d.objects()[0].x).toBe(9)
    expect((d.writes[0].body as { path: string }).path).toBe(PATH)
    expect(boardFileSessionFor(KEY, PATH)).toBeNull()
  })

  it('a held edit outlives the pane (a workspace switch), and the next pane has it', async () => {
    const s = await withHeldEdit()
    await releaseBoardFileSession(KEY, PATH, { discard: false })
    expect(boardFileSessionFor(KEY, PATH)).toBe(s)
    expect(unsavedBoardFiles()).toHaveLength(1)
    const again = acquire()
    expect(again).toBe(s)
    expect(again.view().held).toHaveLength(1)
    expect(again.view().lostNote).toBeNull()
  })

  it('Discard on the leave guard drops queued edits, and sends nothing', async () => {
    const s = acquire()
    await s.start()
    s.submit({ gesture: 't', kind: 'edit', byAll: false, ops: [{ op: 'set', id: 's-a', f: { x: 9 } }] })
    await releaseBoardFileSession(KEY, PATH, { discard: true })
    expect(d.writes).toEqual([])
    expect(boardFileSessionFor(KEY, PATH)).toBeNull()
  })
})

describe('K2 going down without asking', () => {
  it('a reopened tab says the held edit went, and OK clears it', async () => {
    await withHeldEdit()
    // A crash: nothing runs. This window's memory is gone; the note isn't.
    resetBoardFileTabsForTests()
    const s = acquire()
    await s.start()
    expect(s.view().lostNote).toEqual({ edits: 1 })
    expect(s.view().held).toEqual([])
    s.setLostNote(null)
    expect(s.view().lostNote).toBeNull()
  })

  it('no note when the edit was resolved first', async () => {
    const s = await withHeldEdit()
    s.useTheirs(s.view().held[0].id)
    resetBoardFileTabsForTests()
    const again = acquire()
    await again.start()
    expect(again.view().lostNote).toBeNull()
  })
})

describe('the shell’s Quit', () => {
  it('hears when there’s an edit to ask about, and when there isn’t', async () => {
    const s = await withHeldEdit()
    expect(invoke).toHaveBeenCalledWith('set_board_file_unsaved', { unsaved: true })
    s.useTheirs(s.view().held[0].id)
    expect(invoke).toHaveBeenLastCalledWith('set_board_file_unsaved', { unsaved: false })
  })
})
