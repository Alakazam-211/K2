// One open `.k5board` outside its frame: queue and flush, conflicts and held edits, the file's states and Save a copy,
// against a fake daemon that follows board-tab-b3-interface.md. No write route may fire on open, and no edit may go
// missing without the person's say-so.
import { beforeEach, describe, expect, it } from 'vitest'
import { createBoardFileSession, createBoardFilePartJoiner, type BoardFileEvent, type BoardFileSession, type BoardOp } from './board-file-session'
import { createFakeBoardDaemon, httpErr, type FakeBoardDaemon, type FakeObject } from './fake-board-daemon'

const sticky = (id: string, text = 't'): FakeObject => ({ id, type: 'sticky', x: 1, y: 2, w: 200, h: 200, text, color: 'yellow' })

/** Timers under the test's hand: nothing runs until `tick`. */
function clock(): { setTimer(fn: () => void, ms: number): unknown; clearTimer(t: unknown): void; now(): number; tick(ms: number): Promise<void> } {
  let at = 1000
  const timers = new Map<number, { fn: () => void; due: number }>()
  let next = 1
  return {
    setTimer(fn, ms) {
      const id = next++
      timers.set(id, { fn, due: at + ms })
      return id
    },
    clearTimer(t) {
      timers.delete(t as number)
    },
    now: () => at,
    async tick(ms) {
      const end = at + ms
      for (;;) {
        const due = [...timers.entries()].filter(([, v]) => v.due <= end).sort((a, b) => a[1].due - b[1].due)[0]
        if (!due) break
        timers.delete(due[0])
        at = Math.max(at, due[1].due)
        due[1].fn()
        await settle()
      }
      at = end
      await settle()
    },
  }
}
const settle = async (): Promise<void> => {
  for (let i = 0; i < 20; i++) await Promise.resolve()
}

let d: FakeBoardDaemon
let c: ReturnType<typeof clock>
let s: BoardFileSession
let events: BoardFileEvent[]
const saves = (): number => d.writes.filter((w) => w.route === 'board/file/save').length
let seq = 0
const gesture = (ops: BoardOp[], kind: 'edit' | 'undo' | 'redo' = 'edit', byAll = false): string => {
  const t = `t${++seq}`
  s.submit({ gesture: t, kind, byAll, ops })
  return t
}
const eventsOf = <K extends BoardFileEvent['type']>(type: K): Extract<BoardFileEvent, { type: K }>[] =>
  events.filter((e) => e.type === type) as Extract<BoardFileEvent, { type: K }>[]

async function open(objects: FakeObject[] = [sticky('s-a'), sticky('s-b')], opts: Parameters<typeof createFakeBoardDaemon>[0] = {}): Promise<void> {
  d = createFakeBoardDaemon({ objects, ...opts })
  c = clock()
  s = createBoardFileSession({ api: d, path: '/w/Plan.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
  events = []
  s.onEvent((e) => events.push(e))
  await s.start()
}

beforeEach(() => {
  seq = 0
})

describe('opening', () => {
  it('fires no write route: reads, frame open, polls and a fresh event write nothing', async () => {
    await open()
    expect(s.view().phase.kind).toBe('ready')
    s.open()
    await s.pollInfo()
    s.onBoardChanged(d.hash())
    s.hold(true)
    s.hold(false)
    await s.flush()
    await c.tick(60_000)
    expect(d.writes).toEqual([])
    expect(s.pendingCount() + s.heldCount()).toBe(0)
  })

  it('names the board by its file and hands the frame the objects back to front', async () => {
    await open()
    const o = s.open()
    expect(o.name).toBe('Plan')
    expect(o.objects.map((x) => x.id)).toEqual(['s-a', 's-b'])
    expect(o.me).toBe('p-me')
  })

  it('an empty file waits for the person: no save, then Start a board here', async () => {
    d = createFakeBoardDaemon()
    d.setEmpty(true)
    c = clock()
    s = createBoardFileSession({ api: d, path: '/w/E.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
    await s.start()
    expect(s.view().phase.kind).toBe('empty')
    expect(d.writes).toEqual([])
    await s.startBoard()
    expect(d.writes.map((w) => w.route)).toEqual(['board/file/start'])
    expect(s.view().phase.kind).toBe('ready')
  })

  it('a big file asks first, and Open anyway sends allowLarge', async () => {
    d = createFakeBoardDaemon({ objects: [sticky('s-a')] })
    d.setLarge(40 * 1024 * 1024)
    c = clock()
    s = createBoardFileSession({ api: d, path: '/w/B.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
    await s.start()
    expect(s.view().phase).toEqual({ kind: 'large', bytes: 40 * 1024 * 1024 })
    await s.openAnyway()
    expect(s.view().phase.kind).toBe('ready')
    expect(s.open().objects).toHaveLength(1)
  })

  for (const [name, err, kind] of [
    ['a newer format', httpErr(422, 'min_reader_too_high', { needsReader: 3, ours: 1 }), 'newer'],
    ['a corrupt file', httpErr(422, 'zip_bad_entry', { at: 'board.json' }), 'corrupt'],
    ['a bad id', httpErr(422, 'id_invalid', { badIds: ['x'], badIdsTotal: 3, badIdsTruncated: true }), 'corrupt'],
    ['a gone file', httpErr(404, 'not_found'), 'gone'],
    ['no rights', httpErr(403, 'doc_not_shared'), 'denied'],
    ['K2 away', new TypeError('fetch failed'), 'unreachable'],
  ] as const) {
    it(`${name}: a card, never drawn, and no write`, async () => {
      d = createFakeBoardDaemon({ objects: [sticky('s-a')] })
      d.failNext('board/file/read', err)
      c = clock()
      s = createBoardFileSession({ api: d, path: '/w/X.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
      await s.start()
      expect(s.view().phase.kind).toBe(kind)
      expect(d.writes).toEqual([])
    })
  }

  it('the corrupt card carries the code, part and bad ids', async () => {
    d = createFakeBoardDaemon()
    d.failNext('board/file/read', httpErr(422, 'id_invalid', { at: 'board.json', badIds: ['x', 'y'], badIdsTotal: 9, badIdsTruncated: true }))
    c = clock()
    s = createBoardFileSession({ api: d, path: '/w/X.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
    await s.start()
    expect(s.view().phase).toMatchObject({ kind: 'corrupt', code: 'id_invalid', at: 'board.json', badIds: ['x', 'y'], badIdsTotal: 9, badIdsTruncated: true })
  })

  it('a shape above what this build knows says Update K2', async () => {
    d = createFakeBoardDaemon()
    const read = d.get.bind(d)
    d.get = (async (route: string, p: Record<string, string | number | undefined>) => ({ ...(await read<Record<string, unknown>>(route, p)), shape: 2 })) as typeof d.get
    c = clock()
    s = createBoardFileSession({ api: d, path: '/w/X.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
    await s.start()
    expect(s.view().phase.kind).toBe('shape')
  })
})

describe('saving', () => {
  it('flushes after the idle time, with a rev on every op', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 5 } }])
    expect(s.view().status.state).toBe('pending')
    await c.tick(399)
    expect(saves()).toBe(0)
    await c.tick(1)
    expect(saves()).toBe(1)
    const g = (d.writes[0].body.groups as { ops: BoardOp[] }[])[0]
    expect(g.ops[0]).toMatchObject({ op: 'set', id: 's-a', rev: 'r1', f: { x: 5 } })
    expect(s.view().status.state).toBe('saved')
    expect(d.objects()[0].x).toBe(5)
  })

  it('never sends position numbers: z and order stay in the tab', async () => {
    await open()
    gesture([{ op: 'z', id: 's-a', to: 'front', z: 9 }, { op: 'add', obj: sticky('s-c'), at: 'front', z: 10 }])
    await s.flush()
    const ops = (d.writes[0].body.groups as { ops: BoardOp[] }[])[0].ops
    expect(ops[0]).toEqual({ op: 'z', id: 's-a', to: 'front' })
    expect(ops[1]).toEqual({ op: 'add', obj: sticky('s-c'), at: 'front' })
    expect(s.open().objects.map((o) => o.id).at(-1)).toBe('s-c')
  })

  it('holds while typing, and goes when the typing stops', async () => {
    await open()
    s.hold(true)
    gesture([{ op: 'set', id: 's-a', f: { text: 'hi' } }])
    await c.tick(5000)
    expect(saves()).toBe(0)
    s.hold(false)
    await c.tick(400)
    expect(saves()).toBe(1)
  })

  it('Cmd+S goes at once, even while holding', async () => {
    await open()
    s.hold(true)
    gesture([{ op: 'set', id: 's-a', f: { text: 'hi' } }])
    await s.flush()
    expect(saves()).toBe(1)
  })

  it('merges same-object sets only within a gesture: two gestures stay two groups', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 1 } }, { op: 'set', id: 's-a', f: { y: 2 } }])
    gesture([{ op: 'set', id: 's-a', f: { x: 3 } }])
    await s.flush()
    const groups = d.writes.flatMap((w) => w.body.groups as { ops: BoardOp[] }[])
    expect(groups[0].ops).toHaveLength(1)
    expect(groups[0].ops[0].f).toEqual({ x: 1, y: 2 })
    expect(groups[1].ops[0].f).toEqual({ x: 3 })
    expect(s.heldCount()).toBe(0)
  })

  it('an op made during a save takes the new rev, so the tab never conflicts with itself', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 1 } }])
    const release = d.holdSaves()
    await c.tick(400)
    expect(s.view().status.state).toBe('saving')
    gesture([{ op: 'set', id: 's-a', f: { x: 2 } }])
    release()
    await settle()
    await c.tick(0)
    await s.flush()
    expect(saves()).toBe(2)
    expect((d.writes[1].body.groups as { ops: BoardOp[] }[])[0].ops[0].rev).toBe('r3')
    expect(s.heldCount()).toBe(0)
    expect(d.objects()[0].x).toBe(2)
  })

  it('a set on an object added here waits for its rev, then goes', async () => {
    await open()
    gesture([{ op: 'add', obj: sticky('s-new'), at: 'front', z: 3 }])
    gesture([{ op: 'set', id: 's-new', f: { text: 'later' } }])
    await s.flush()
    expect(saves()).toBe(2)
    expect((d.writes[1].body.groups as { ops: BoardOp[] }[])[0].ops[0].rev).toBeTruthy()
    expect(d.objects().find((o) => o.id === 's-new')?.text).toBe('later')
  })

  it('a meta op carries the settings rev', async () => {
    await open()
    gesture([{ op: 'meta', f: { voting: { active: true } } }])
    await s.flush()
    expect((d.writes[0].body.groups as { ops: BoardOp[] }[])[0].ops[0].rev).toBe('s1')
  })

  it('byAll goes on the group', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { votes: {} } }], 'edit', true)
    await s.flush()
    expect((d.writes[0].body.groups as { byAll: boolean }[])[0].byAll).toBe(true)
  })

  it('undo of a delete sends the raw object K2 gave back, never one rebuilt from the widget', async () => {
    await open([{ ...sticky('s-a'), x: 1.25, Foo: 9 }, sticky('s-b')])
    gesture([{ op: 'del', id: 's-a' }])
    await s.flush()
    gesture([{ op: 'add', obj: { ...sticky('s-a'), x: 1.3 }, at: 'back', z: 0 }], 'undo')
    await s.flush()
    const ops = (d.writes[1].body.groups as { ops: BoardOp[]; noHold: boolean }[])[0]
    expect(ops.noHold).toBe(true)
    expect(ops.ops[0].op).toBe('putback')
    expect((ops.ops[0].object as FakeObject).x).toBe(1.25)
    expect((ops.ops[0].object as FakeObject).Foo).toBe(9)
    expect(d.objects().find((o) => o.id === 's-a')?.x).toBe(1.25)
  })

  it('a delete undone before it left sends nothing at all', async () => {
    await open()
    const a = gesture([{ op: 'del', id: 's-a' }])
    const b = gesture([{ op: 'add', obj: sticky('s-a'), at: 'back', z: 0 }], 'undo')
    await s.flush()
    await c.tick(10_000)
    expect(saves()).toBe(0)
    expect(eventsOf('saved').flatMap((e) => e.tokens)).toEqual([a, b])
  })

  it('a refused group rolls back in the frame and the file’s truth returns; other groups go on', async () => {
    await open()
    d.setSaveRefusal({ code: 'object_locked', hint: 'locked' })
    const t = gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    await s.flush()
    await settle()
    expect(eventsOf('refused')[0].gesture).toBe(t)
    expect(s.open().objects[0].x).toBe(1)
    expect(s.pendingCount()).toBe(0)
    d.setSaveRefusal(null)
    gesture([{ op: 'set', id: 's-b', f: { x: 7 } }])
    await s.flush()
    expect(d.objects()[1].x).toBe(7)
  })

  it('a gesture larger than one message joins, in order', () => {
    const join = createBoardFilePartJoiner()
    expect(join({ gesture: 't', part: 0, last: false, text: '[1,' })).toBeNull()
    expect(join({ gesture: 't', part: 1, last: true, text: '2]' })).toBe('[1,2]')
    expect(() => join({ gesture: 'u', part: 1, last: true, text: 'x' })).toThrow(/out of order/)
    const small = createBoardFilePartJoiner(3)
    expect(() => small({ gesture: 't', part: 0, last: true, text: 'abcd' })).toThrow(/too large/)
  })
})

describe('another writer', () => {
  it('an agent’s change arrives as a remote op, not undoable, and writes nothing', async () => {
    await open()
    d.outsideSet('s-a', { text: 'agent' })
    s.onBoardChanged(d.hash())
    await settle()
    expect(eventsOf('remote')[0].ops).toEqual([{ op: 'set', id: 's-a', f: { text: 'agent' } }])
    expect(d.writes).toEqual([])
    expect(s.open().objects[0].text).toBe('agent')
  })

  it('its own save’s event is ignored by hash', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 5 } }])
    await s.flush()
    const reads = d.calls.filter((r) => r === 'board/file/read').length
    s.onBoardChanged(d.hash())
    await settle()
    expect(d.calls.filter((r) => r === 'board/file/read').length).toBe(reads)
  })

  it('an outside change shows through info then read, with adds, deletes and a new order', async () => {
    await open()
    d.outsideAdd(sticky('s-c'))
    d.outsideDelete('s-b')
    s.onOutsideChange()
    await settle()
    const ops = eventsOf('remote').flatMap((e) => e.ops)
    expect(ops.some((o) => o.op === 'add' && (o.obj as FakeObject).id === 's-c')).toBe(true)
    expect(ops).toContainEqual({ op: 'del', id: 's-b' })
    expect(s.open().objects.map((o) => o.id)).toEqual(['s-a', 's-c'])
  })

  it('an outside add in the middle moves the order without touching anyone’s values', async () => {
    await open()
    const read = d.get.bind(d)
    d.outsideAdd(sticky('s-c'))
    d.get = (async (route: string, p: Record<string, string | number | undefined>) => {
      const r = await read<{ objects?: FakeObject[] }>(route, p)
      if (r.objects) r.objects = [r.objects[0], r.objects[2], r.objects[1]].map((o, i) => ({ ...o, z: i + 1 }))
      return r
    }) as typeof d.get
    s.onOutsideChange()
    await settle()
    const ops = eventsOf('remote').flatMap((e) => e.ops)
    expect(ops).toContainEqual({ op: 'set', id: 's-b', f: { z: 3 } })
    expect(s.open().objects.map((o) => o.id)).toEqual(['s-a', 's-c', 's-b'])
  })

  it('a read never overruns a save in flight: it waits for the answer', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 1 } }])
    const release = d.holdSaves()
    await c.tick(400)
    const reads = d.calls.filter((r) => r === 'board/file/read').length
    d.outsideSet('s-b', { text: 'agent' })
    s.onBoardChanged('sha256-other')
    await settle()
    expect(d.calls.filter((r) => r === 'board/file/read').length).toBe(reads)
    release()
    await settle()
    await settle()
    expect(s.open().objects[1].text).toBe('agent')
  })
})

describe('conflicts: nothing resolves itself', () => {
  async function conflict(): Promise<void> {
    await open()
    d.outsideSet('s-a', { text: 'theirs' })
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
  }

  it('holds the edit, says so on the chip, shows theirs, and lists the fields', async () => {
    await conflict()
    expect(s.heldCount()).toBe(1)
    const view = s.view()
    expect(view.status).toMatchObject({ state: 'error', text: '1 edit isn’t saved' })
    expect(view.held[0].objects[0]).toMatchObject({ id: 's-a', why: 'changed' })
    expect(view.held[0].objects[0].fields).toEqual([{ field: 'text', mine: 'mine', theirs: 'theirs' }])
    expect(d.objects()[0].text).toBe('theirs')
    expect(s.open().objects[0].text).toBe('theirs')
  })

  it('stays held over any number of saves and polls, until the person picks', async () => {
    await conflict()
    await c.tick(120_000)
    await s.pollInfo()
    expect(s.heldCount()).toBe(1)
    expect(d.objects()[0].text).toBe('theirs')
  })

  it('Use mine resends with their rev, and the file takes it', async () => {
    await conflict()
    s.useMine(s.view().held[0].id)
    await s.flush()
    expect(d.objects()[0].text).toBe('mine')
    expect(s.heldCount()).toBe(0)
    expect(s.open().objects[0].text).toBe('mine')
    expect(eventsOf('remote').flatMap((e) => e.ops)).toContainEqual({ op: 'set', id: 's-a', f: { text: 'mine' } })
  })

  it('Use theirs drops the edit and writes nothing', async () => {
    await conflict()
    const before = saves()
    s.useTheirs(s.view().held[0].id)
    await c.tick(10_000)
    expect(saves()).toBe(before)
    expect(s.heldCount()).toBe(0)
    expect(d.objects()[0].text).toBe('theirs')
  })

  it('a deleted one: Put it back makes it again, Let it go leaves it gone', async () => {
    await open()
    d.outsideDelete('s-a')
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
    const view = s.view()
    expect(view.held[0].objects[0].why).toBe('deleted')
    expect(s.open().objects.map((o) => o.id)).toEqual(['s-b'])
    s.useMine(view.held[0].id)
    await s.flush()
    expect(d.objects().find((o) => o.id === 's-a')?.text).toBe('mine')
  })

  it('a stale rev with a current base still conflicts: every rev is compared', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 1 } }])
    await s.flush()
    // The queued op's rev is what it was at the gesture, whatever the base hash says.
    d.outsideSet('s-a', { x: 2 })
    gesture([{ op: 'set', id: 's-a', f: { x: 3 } }])
    await s.flush()
    await settle()
    expect(s.heldCount()).toBe(1)
    expect(d.objects()[0].x).toBe(2)
  })

  it('an undo that meets a change it hadn’t seen comes back as historyRefused, and is not held', async () => {
    await open()
    d.outsideSet('s-a', { x: 2 })
    const t = gesture([{ op: 'set', id: 's-a', f: { x: 1 } }], 'undo')
    await s.flush()
    await settle()
    expect(eventsOf('historyRefused')[0].gesture).toBe(t)
    expect(s.heldCount()).toBe(0)
    expect(d.objects()[0].x).toBe(2)
  })

  it('a partial save: 200 with the held group, and the rest saved', async () => {
    await open()
    d.outsideSet('s-a', { text: 'theirs' })
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    gesture([{ op: 'set', id: 's-b', f: { text: 'fine' } }])
    await s.flush()
    await settle()
    expect(s.heldCount()).toBe(1)
    expect(d.objects()[1].text).toBe('fine')
  })
})

describe('the file’s states', () => {
  it('moved or deleted: read only, edits kept, nothing recreated; clears when it comes back', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failAll(httpErr(404, 'not_found'))
    await s.flush()
    expect(s.view().file).toBe('moved')
    expect(s.view().readOnlyWhy).toMatch(/moved or deleted/)
    expect(s.pendingCount()).toBe(1)
    expect(eventsOf('readOnly').at(-1)).toMatchObject({ on: true })
    const tries = (): number => d.calls.filter((r) => r === 'board/file/save').length
    const n = tries()
    await c.tick(30_000)
    // Not by a timer, Cmd+S or a new edit: K2 never recreates a file that's gone.
    await s.flush()
    gesture([{ op: 'set', id: 's-b', f: { x: 1 } }])
    await c.tick(30_000)
    expect(tries()).toBe(n)
    d.failAll(null)
    await s.pollInfo()
    await settle()
    await s.flush()
    expect(s.view().file).toBe('ok')
    expect(d.objects()[0].x).toBe(9)
    expect(eventsOf('readOnly').at(-1)).toMatchObject({ on: false })
  })

  it('goes bad while open: keeps board and edits, retries, then offers a copy', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failAll(httpErr(422, 'zip_bad_entry'))
    await s.pollInfo()
    expect(s.view().file).toBe('unreadable')
    await c.tick(70_000)
    expect(s.view().file).toBe('unreadable-long')
    expect(s.pendingCount()).toBe(1)
    expect(d.writes).toEqual([])
    d.failAll(null)
    await s.pollInfo()
    await settle()
    await s.flush()
    expect(s.view().file).toBe('ok')
    expect(d.objects()[0].x).toBe(9)
  })

  it('disk full: edits stay queued, the chip says why, and it retries every 5 s', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failNext('board/file/save', httpErr(503, 'disk_full'), 2)
    await s.flush()
    expect(s.pendingCount()).toBe(1)
    expect(s.view().status).toMatchObject({ state: 'error' })
    expect(s.view().status.text).toMatch(/disk is full/)
    await c.tick(5_000)
    expect(s.pendingCount()).toBe(1)
    await c.tick(5_000)
    expect(s.pendingCount()).toBe(0)
    expect(d.objects()[0].x).toBe(9)
  })

  it('K2 gone: edits stay queued, and the same group ids retry', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failNext('board/file/save', 'network')
    await s.flush()
    expect(s.view().status.text).toMatch(/isn’t answering/)
    const first = (d.calls.length, s.pendingCount())
    expect(first).toBe(1)
    await c.tick(5_000)
    expect(d.objects()[0].x).toBe(9)
  })

  it('a retry while hidden waits for the tab to show', async () => {
    d = createFakeBoardDaemon({ objects: [sticky('s-a')] })
    c = clock()
    let shown = false
    s = createBoardFileSession({ api: d, path: '/w/P.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => shown })
    await s.start()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failNext('board/file/save', 'network')
    await s.flush()
    await c.tick(20_000)
    expect(s.pendingCount()).toBe(1)
    shown = true
    await c.tick(5_000)
    expect(s.pendingCount()).toBe(0)
  })

  it('a read-only folder: no save is tried, the tab says why, and edits stay', async () => {
    await open([sticky('s-a')], { canWrite: false })
    expect(s.view().readOnlyWhy).toMatch(/read only/)
    expect(s.open().readOnly).toMatch(/read only/)
    await s.flush()
    expect(d.writes).toEqual([])
  })

  it('503 read_only on a save: edits stay queued and the chip says why', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failNext('board/file/save', httpErr(503, 'read_only'))
    await s.flush()
    expect(s.view().readOnlyWhy).toMatch(/read only/)
    expect(s.pendingCount()).toBe(1)
  })

  it('a dangling picture: lists it; a refused save holds until the person clears it, with the count', async () => {
    await open([sticky('s-a')], { missing: ['sha256-gone'] })
    expect(s.view().missing).toEqual(['sha256-gone'])
    d.setSaveRefusal({ code: 'blob_missing' })
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    await s.flush()
    expect(s.view().missingBlocksSave).toBe(true)
    expect(s.pendingCount()).toBe(1)
    d.setSaveRefusal(null)
    s.clearMissing()
    await s.flush()
    const ops = d.writes.flatMap((w) => (w.body.groups as { ops: BoardOp[] }[]).flatMap((g) => g.ops))
    expect(ops).toContainEqual({ op: 'clearBlob', hash: 'sha256-gone' })
    expect(d.objects()[0].x).toBe(9)
  })
})

describe('Save a copy, with the original location as the problem', () => {
  it('file readable: the daemon copies, with the queued edits, and the original is left alone', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    const r = await s.saveCopy('/other/Plan copy.k5board')
    expect(r).toMatchObject({ ok: true, path: '/other/Plan copy.k5board' })
    expect(d.copies[0].kind).toBe('copy')
    const groups = d.copies[0].body.groups as { ops: BoardOp[] }[]
    expect(groups[0].ops[0]).toMatchObject({ op: 'set', id: 's-a', f: { x: 9 }, rev: 'r1' })
    expect(d.objects()[0].x).toBe(1)
    expect(s.view().copiedTo).toBe('/other/Plan copy.k5board')
  })

  it('full disk: saves stay queued, and a copy goes elsewhere with those edits', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failNext('board/file/save', httpErr(503, 'disk_full'), 100)
    await s.flush()
    expect(s.pendingCount()).toBe(1)
    const r = await s.saveCopy('/roomy/Plan.k5board')
    expect(r.ok).toBe(true)
    expect((d.copies[0].body.groups as { ops: BoardOp[] }[])[0].ops[0]).toMatchObject({ f: { x: 9 } })
  })

  it('gone share: the tab’s own board goes out, in order, with its edits and the pictures it still holds', async () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 1, 2, 3])
    await open([{ ...sticky('i-a'), type: 'image', src: 'k2blob:sha256-abc' }, sticky('s-b')])
    s.rememberBlob('sha256-abc', 'image/png', png)
    gesture([{ op: 'set', id: 's-b', f: { text: 'edit' } }])
    d.failAll(httpErr(404, 'not_found'))
    await s.flush()
    expect(s.view().file).toBe('moved')
    d.failAll(null)
    d.failNext('board/file/read', httpErr(404, 'not_found'), 100)
    const r = await s.saveCopy('/usb/Plan.k5board')
    expect(r).toMatchObject({ ok: true, path: '/usb/Plan.k5board' })
    const rescue = d.copies[0]
    expect(rescue.kind).toBe('rescue')
    expect((rescue.body.objects as FakeObject[]).map((o) => o.id)).toEqual(['i-a', 's-b'])
    expect((rescue.body.objects as FakeObject[])[1].text).toBe('edit')
    expect(rescue.body.blobs).toEqual(['sha256-abc'])
    expect(d.staged.has('sha256-abc')).toBe(true)
    expect(s.pendingCount()).toBe(1)
  })

  it('gone share with a picture it never loaded: asks, and only then saves without it', async () => {
    await open([{ ...sticky('i-a'), type: 'image', src: 'k2blob:sha256-never' }])
    d.failAll(httpErr(404, 'not_found'))
    await s.pollInfo()
    expect(s.view().file).toBe('moved')
    d.failAll(null)
    const asked = await s.saveCopy('/usb/P.k5board')
    expect(asked).toEqual({ ok: false, missing: ['sha256-never'] })
    expect(d.copies).toEqual([])
    const done = await s.saveCopy('/usb/P.k5board', { dropMissing: true })
    expect(done).toMatchObject({ ok: true, missing: ['sha256-never'] })
    expect(d.copies[0].body.dropMissing).toBe(true)
  })

  it('the copy route turning out not_found falls back to the tab’s own board', async () => {
    await open()
    d.failNext('board/file/copy', httpErr(404, 'not_found'))
    const r = await s.saveCopy('/usb/P.k5board')
    expect(r.ok).toBe(true)
    expect(d.copies[0].kind).toBe('rescue')
  })

  it('a copy that can’t land says so, and nothing is lost', async () => {
    await open()
    gesture([{ op: 'set', id: 's-a', f: { x: 9 } }])
    d.failNext('board/file/copy', httpErr(503, 'disk_full'))
    await expect(s.saveCopy('/full/P.k5board')).rejects.toThrow(/disk_full/)
    expect(s.pendingCount()).toBe(1)
    expect(s.view().copiedTo).toBeNull()
  })

  it('held edits go in the copy too, on the current revs', async () => {
    await open()
    d.outsideSet('s-a', { text: 'theirs' })
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
    expect(s.heldCount()).toBe(1)
    await s.saveCopy('/usb/P.k5board')
    const g = d.copies[0].body.groups as { ops: BoardOp[]; noHold: boolean }[]
    expect(g[0].ops[0]).toMatchObject({ f: { text: 'mine' } })
    expect(g[0].ops[0].rev).toBe('r3')
  })
})

describe('held edits the daemon keeps: they survive this tab, a restart and a crash', () => {
  async function conflict(): Promise<void> {
    await open()
    d.storeHeld(true)
    d.outsideSet('s-a', { text: 'theirs' })
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
  }
  /** K2 restarted: nothing of this tab's memory is left, only the daemon's store. */
  async function restart(): Promise<void> {
    s.dispose()
    c = clock()
    s = createBoardFileSession({ api: d, path: '/w/Plan.k5board', setTimer: c.setTimer, clearTimer: c.clearTimer, now: c.now, visible: () => true })
    events = []
    s.onEvent((e) => events.push(e))
    await s.start()
  }

  it('the chip still says it isn’t in the file, and the leave guard needn’t ask', async () => {
    await conflict()
    expect(s.view().held).toHaveLength(1)
    expect(s.view().held[0].heldId).toBe('h-1')
    expect(s.view().status).toMatchObject({ state: 'error', text: '1 edit isn’t saved' })
    expect(s.heldCount()).toBe(0)
  })

  it('comes back after a restart, with both sides of each field', async () => {
    await conflict()
    await restart()
    const h = s.view().held
    expect(h).toHaveLength(1)
    expect(h[0].objects[0]).toMatchObject({ id: 's-a', why: 'changed' })
    expect(h[0].objects[0].fields).toEqual([{ field: 'text', mine: 'mine', theirs: 'theirs' }])
  })

  it('a read while it’s held doesn’t lose or double it', async () => {
    await conflict()
    d.outsideSet('s-b', { text: 'other' })
    s.onBoardChanged('sha256-other')
    await settle()
    expect(s.view().held).toHaveLength(1)
  })

  it('Use mine after a restart resends with their rev; the store clears when it lands', async () => {
    await conflict()
    await restart()
    s.useMine(s.view().held[0].id)
    await s.flush()
    await settle()
    expect(d.objects()[0].text).toBe('mine')
    const g = d.writes.filter((w) => w.route === 'board/file/save').at(-1)!.body.groups as { heldId?: string; ops: BoardOp[] }[]
    expect(g[0].heldId).toBe('h-1')
    expect(g[0].ops[0].rev).toBe(`r${3}`)
    expect(d.stored()).toEqual([])
    expect(s.view().held).toEqual([])
  })

  it('Use mine that conflicts again stays held, under the same id', async () => {
    await conflict()
    await restart()
    d.outsideSet('s-a', { text: 'theirs again' })
    s.useMine(s.view().held[0].id)
    await s.flush()
    await settle()
    expect(d.stored()).toHaveLength(1)
    expect(s.view().held).toHaveLength(1)
    expect(s.view().held[0].heldId).toBe('h-1')
    expect(d.objects()[0].text).toBe('theirs again')
  })

  it('Use theirs clears the store, and never calls save', async () => {
    await conflict()
    const before = saves()
    s.useTheirs(s.view().held[0].id)
    await settle()
    expect(saves()).toBe(before)
    expect(d.stored()).toEqual([])
    await restart()
    expect(s.view().held).toEqual([])
  })

  it('a clear that fails comes back, never lost', async () => {
    await conflict()
    d.failNext('board/file/held/resolve', 'network')
    s.useTheirs(s.view().held[0].id)
    await settle()
    expect(d.stored()).toHaveLength(1)
    expect(s.view().held).toHaveLength(1)
  })

  it('a deleted object: its whole self goes to the store, so Put it back works after a restart', async () => {
    await open()
    d.storeHeld(true)
    d.outsideDelete('s-a')
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
    expect(d.stored()[0].mine).toBeTruthy()
    await restart()
    const h = s.view().held[0]
    expect(h.objects[0].why).toBe('deleted')
    expect(h.objects[0].mine?.id).toBe('s-a')
    s.useMine(h.id)
    await s.flush()
    await settle()
    expect(d.objects().find((o) => o.id === 's-a')).toBeTruthy()
    expect(d.stored()).toEqual([])
  })

  it('a deleted object the daemon didn’t get: Put it back does nothing rather than clear the edit', async () => {
    await open()
    d.storeHeld(true)
    d.outsideDelete('s-a')
    d.failNext('board/file/held/mine', httpErr(413, 'held_full'))
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
    expect(d.stored()[0].mine).toBeUndefined()
    await restart()
    const h = s.view().held[0]
    expect(h.objects[0].mine).toBeNull()
    expect(h.objects[0].fields).toEqual([{ field: 'text', mine: 'mine', theirs: '(none)' }])
    s.useMine(h.id)
    await settle()
    expect(s.view().held).toHaveLength(1)
    expect(d.stored()).toHaveLength(1)
    s.useTheirs(h.id)
    await settle()
    expect(d.stored()).toEqual([])
  })

  it('held_full: the entry has no heldId, so it’s memory-only and the guards ask', async () => {
    await open()
    d.outsideSet('s-a', { text: 'theirs' })
    const post = d.post.bind(d)
    d.post = (async (route: string, body: unknown) => {
      try {
        const r = await post<{ held?: Record<string, unknown>[] }>(route, body)
        return r
      } catch (err) {
        const b = (err as { body?: { held?: Record<string, unknown>[] } }).body
        for (const h of b?.held ?? []) h.heldError = 'held_full'
        throw err
      }
    }) as typeof d.post
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
    expect(s.view().held[0].heldId).toBeNull()
    expect(s.heldCount()).toBe(1)
  })

  it('a daemon that doesn’t keep them: held in memory, and the guards still ask', async () => {
    await open()
    d.outsideSet('s-a', { text: 'theirs' })
    gesture([{ op: 'set', id: 's-a', f: { text: 'mine' } }])
    await s.flush()
    await settle()
    expect(s.view().held[0].heldId).toBeNull()
    expect(s.heldCount()).toBe(1)
  })
})
