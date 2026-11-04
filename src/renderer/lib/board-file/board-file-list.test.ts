// @vitest-environment jsdom
//
// The Boards Garden's file list (Rosson 2026-11-03, a preview): the daemon's rows re-listed on its change events,
// debounced and never on a timer; the two verbs only K2's own Board widget gets, and only with board files on.
import { describe, expect, it } from 'vitest'
import { createZenCustomLayer, type ZenCustomBoardFiles, type ZenCustomLayerDeps } from '@/lib/zen/zen-custom-bridge'
import type { ZenCustomWidgetPayload } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZEN_FRAME_FEATURES, zenFrameFeatures, zenListsBoardFiles } from '@/components/Zen/widgets/ZenCustomFrame'
import {
  BOARD_FILE_LIST_FEATURE,
  BOARD_FILE_LIST_WIDGET,
  BOARD_FILE_RELIST_MS,
  createBoardFileListing,
  parseBoardFileList,
  type BoardFileFrameList,
} from './board-file-list'

const row = (name: string, ws = '/w/plans', wsName = 'plans') => ({
  path: `${ws}/${name}.k5board`,
  rel: `${name}.k5board`,
  name,
  workspace: ws,
  workspaceName: wsName,
  bytes: 10,
  mtimeMs: 1,
})

/** A daemon stand-in: the route's answers in order, the two event buses, and a clock the test moves. */
function rig(answers: Array<unknown | Error>) {
  const fetched: number[] = []
  const saved = new Set<() => void>()
  const fs = new Set<(paths: readonly string[]) => void>()
  const timers = new Map<number, { fn: () => void; at: number }>()
  let now = 0
  let next = 1
  const listing = createBoardFileListing({
    load: async () => {
      fetched.push(now)
      const a = answers.length > 1 ? answers.shift() : answers[0]
      if (a instanceof Error) throw a
      return a
    },
    onBoardFileChanged: (fn) => {
      saved.add(fn)
      return () => saved.delete(fn)
    },
    onFsChanged: (fn) => {
      fs.add(fn)
      return () => fs.delete(fn)
    },
    setTimer: (fn, ms) => {
      const id = next++
      timers.set(id, { fn, at: now + ms })
      return id
    },
    clearTimer: (t) => {
      timers.delete(t as number)
    },
  })
  const flush = () => new Promise<void>((r) => setTimeout(r, 0))
  return {
    listing,
    fetched,
    saved,
    fs,
    timers,
    boardFileChanged: () => [...saved].forEach((f) => f()),
    fsChanged: (paths: string[]) => [...fs].forEach((f) => f(paths)),
    async advance(ms: number) {
      now += ms
      for (const [id, t] of [...timers]) {
        if (t.at <= now) {
          timers.delete(id)
          t.fn()
        }
      }
      await flush()
    },
    flush,
  }
}

describe('parseBoardFileList', () => {
  it('keeps the names-only fields, and refuses a row without them', () => {
    expect(parseBoardFileList({ boards: [row('Plan')], truncated: false })).toEqual({
      boards: [{ path: '/w/plans/Plan.k5board', rel: 'Plan.k5board', name: 'Plan', workspace: '/w/plans', workspaceName: 'plans' }],
      truncated: false,
    })
    expect(() => parseBoardFileList({ boards: [{ path: '/w/a.k5board' }], truncated: false })).toThrow(/row 0/)
    expect(() => parseBoardFileList({ boards: [] })).toThrow(/truncated/)
    expect(() => parseBoardFileList(null)).toThrow(/not an object/)
  })
})

describe('the listing', () => {
  it('pushes the list at once, an empty one too', async () => {
    const r = rig([{ boards: [], truncated: false }])
    const got: BoardFileFrameList[] = []
    r.listing.subscribe((l) => got.push(l), (e) => { throw e })
    await r.flush()
    expect(got).toEqual([{ boards: [], truncated: false }])
    expect(r.timers.size).toBe(0)
  })

  it('re-lists on board_file_changed, debounced: a burst is one read', async () => {
    const r = rig([{ boards: [row('Plan')], truncated: false }, { boards: [row('Plan'), row('Roadmap')], truncated: false }])
    const got: BoardFileFrameList[] = []
    r.listing.subscribe((l) => got.push(l), (e) => { throw e })
    await r.flush()
    expect(r.fetched).toHaveLength(1)
    r.boardFileChanged()
    r.boardFileChanged()
    r.boardFileChanged()
    await r.advance(BOARD_FILE_RELIST_MS - 1)
    expect(r.fetched).toHaveLength(1)
    await r.advance(1)
    expect(r.fetched).toHaveLength(2)
    expect(got).toEqual([
      { boards: [{ id: 'bf-1', name: 'Plan', rel: 'Plan.k5board', workspaceName: 'plans' }], truncated: false },
      {
        boards: [
          { id: 'bf-1', name: 'Plan', rel: 'Plan.k5board', workspaceName: 'plans' },
          { id: 'bf-2', name: 'Roadmap', rel: 'Roadmap.k5board', workspaceName: 'plans' },
        ],
        truncated: false,
      },
    ])
    // No absolute path reaches the frame; the listing keeps it behind the id.
    expect(JSON.stringify(got)).not.toContain('/w/plans')
    expect(r.listing.row('bf-2')?.path).toBe('/w/plans/Roadmap.k5board')
    expect(r.listing.row('bf-9')).toBeNull()
  })

  it('re-lists on an fs_changed that names a .k5board, and not on any other', async () => {
    const r = rig([{ boards: [], truncated: false }])
    r.listing.subscribe(() => undefined, (e) => { throw e })
    await r.flush()
    r.fsChanged(['/w/plans/src/main.rs', '/w/plans/notes.md'])
    await r.advance(BOARD_FILE_RELIST_MS)
    expect(r.fetched).toHaveLength(1)
    r.fsChanged(['/w/plans/New.K5BOARD'])
    await r.advance(BOARD_FILE_RELIST_MS)
    expect(r.fetched).toHaveLength(2)
  })

  it('pushes again only when the list changed', async () => {
    const r = rig([{ boards: [row('Plan')], truncated: false }])
    let pushes = 0
    r.listing.subscribe(() => pushes++, (e) => { throw e })
    await r.flush()
    r.boardFileChanged()
    await r.advance(BOARD_FILE_RELIST_MS)
    expect(r.fetched).toHaveLength(2)
    expect(pushes).toBe(1)
  })

  it('a refused first read fails the subscription and stops listening', async () => {
    const r = rig([new Error('404 not_enabled')])
    const fails: unknown[] = []
    r.listing.subscribe(() => { throw new Error('no list expected') }, (e) => fails.push(e))
    await r.flush()
    expect(fails).toHaveLength(1)
    expect(String(fails[0])).toMatch(/not_enabled/)
    expect(r.saved.size).toBe(0)
    expect(r.fs.size).toBe(0)
  })

  it('a stop ends the listening and any re-list waiting', async () => {
    const r = rig([{ boards: [], truncated: false }])
    const stop = r.listing.subscribe(() => undefined, (e) => { throw e })
    await r.flush()
    r.boardFileChanged()
    stop()
    expect(r.timers.size).toBe(0)
    expect(r.saved.size).toBe(0)
    await r.advance(BOARD_FILE_RELIST_MS)
    expect(r.fetched).toHaveLength(1)
  })
})

describe('the switch and the widget', () => {
  it('only K2’s own Board lists files, and only with board files on; otherwise the hello is today’s', () => {
    const on = () => true
    const off = () => false
    expect(zenListsBoardFiles(BOARD_FILE_LIST_WIDGET, on)).toBe(true)
    expect(zenListsBoardFiles(BOARD_FILE_LIST_WIDGET, off)).toBe(false)
    expect(zenListsBoardFiles('my-board', on)).toBe(false)
    expect(zenListsBoardFiles('k2:kanban@1', on)).toBe(false)
    expect(zenFrameFeatures(false)).toEqual([...ZEN_FRAME_FEATURES])
    expect(zenFrameFeatures(true)).toEqual([...ZEN_FRAME_FEATURES, BOARD_FILE_LIST_FEATURE])
    expect(ZEN_FRAME_FEATURES).not.toContain(BOARD_FILE_LIST_FEATURE)
  })
})

const CAPS = ['store:read', 'store:write', 'links:open']
const widget = { id: 'board', kind: 'custom', widget: BOARD_FILE_LIST_WIDGET, column: 0, props: { config: {} }, name: 'Board', hash: 'h', caps: CAPS, requested: CAPS, paused: null } as unknown as ZenCustomWidgetPayload

function layer(boardFiles?: ZenCustomBoardFiles, push: (sub: number, v: unknown) => void = () => undefined, more: Partial<ZenCustomLayerDeps> = {}) {
  const deps: ZenCustomLayerDeps = {
    widget: () => widget,
    gardenId: () => 'g-test0001',
    inner: new Proxy({}, { get: () => () => { throw new Error('no inner bridge here') } }) as ZenWidgetBridge,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({ theme: { scheme: 'light', vars: {} }, chrome: { corners: null, stoplights: null }, motion: { reduced: false } }),
    onThemeChange: () => () => undefined,
    now: () => Date.now(),
    ...(boardFiles ? { boardFiles } : {}),
    ...more,
  }
  return createZenCustomLayer(deps)
}
const call = (l: ReturnType<typeof layer>, verb: string, args: unknown[] = []) =>
  l.handle({ id: 1, verb, args }) as Promise<{ value?: unknown; error?: { code: string } }>

describe('the layer', () => {
  it('without the listing (any other widget, the switch off) both verbs are unavailable', async () => {
    const l = layer()
    expect((await call(l, 'boardFile.openTab', ['bf-1'])).error?.code).toBe('verb_unavailable')
    expect(await l.handle({ sub: 7, verb: 'boardFile.files', args: [] })).toEqual({ sub: 7, error: expect.objectContaining({ code: 'verb_unavailable' }) })
  })

  it('pushes the listing’s rows, and opens a tab only just after a press', async () => {
    const pushes: unknown[] = []
    const opened: string[] = []
    let pressed = false
    const l = layer(
      {
        files: (push) => {
          push({ boards: [], truncated: false })
          return () => undefined
        },
        pressed: () => pressed,
        openTab: async (path) => {
          opened.push(path)
          return true
        },
      },
      (_sub, v) => pushes.push(v),
    )
    expect(await l.handle({ sub: 2, verb: 'boardFile.files', args: [] })).toBeNull()
    expect(pushes).toEqual([{ boards: [], truncated: false }])
    expect((await call(l, 'boardFile.openTab', ['bf-1'])).error?.code).toBe('failed')
    expect(opened).toEqual([])
    pressed = true
    expect((await call(l, 'boardFile.openTab', [42])).error?.code).toBe('bad_args')
    const r = await call(l, 'boardFile.openTab', ['bf-1'])
    expect(r.value).toEqual({ opened: true })
    expect(opened).toEqual(['bf-1'])
  })

  it('a refused listing reaches the widget as a refusal', async () => {
    const errors: Array<{ sub: number; code: string }> = []
    let refuse: (err: unknown) => void = () => { throw new Error('the layer never subscribed') }
    const l = layer(
      {
        files: (_push, fail) => {
          refuse = fail
          return () => undefined
        },
        pressed: () => true,
        openTab: async () => true,
      },
      () => undefined,
      { pushError: (sub, e) => errors.push({ sub, code: e.code }) },
    )
    expect(await l.handle({ sub: 3, verb: 'boardFile.files', args: [] })).toBeNull()
    refuse(new Error('404 not_enabled'))
    expect(errors).toEqual([{ sub: 3, code: 'failed' }])
  })
})
