// @vitest-environment jsdom
//
// What a board file's sealed frame can reach: the layer a tab builds answers only its own few verbs, and a Garden frame
// gets `verb_unavailable` for all of `boardFile.*`. And the file's own parts: a manifest can lie, so only what sniffs as
// a picture crosses to an `<img>`.
import { describe, expect, it, vi } from 'vitest'
import { createZenCustomLayer, type ZenCustomBoardFile, type ZenCustomLayerDeps } from '@/lib/zen/zen-custom-bridge'
import type { ZenCustomWidgetPayload } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZEN_FRAME_FEATURES } from '@/components/Zen/widgets/ZenCustomFrame'
import { BOARD_FILE_FEATURES, BOARD_FILE_WIDGET, boardFileConsentKey, boardFileTheme, redispatchBoardFileKey } from './board-file-frame'
import { createBoardFileBlobs } from './board-file-blobs'
import { createBoardFileSession } from './board-file-session'
import { createFakeBoardDaemon } from './fake-board-daemon'

const CAPS = ['store:read', 'store:write', 'links:open', 'downloads:save', 'uploads:pick']
const widget = { id: 'board-file:tab', kind: 'custom', widget: BOARD_FILE_WIDGET, column: 0, props: { config: {} }, name: 'Board', hash: 'h', caps: CAPS, requested: CAPS, paused: null } as unknown as ZenCustomWidgetPayload

function layer(boardFile?: ZenCustomBoardFile, push: (sub: number, v: unknown) => void = () => undefined): ReturnType<typeof createZenCustomLayer> {
  const deps: ZenCustomLayerDeps = {
    widget: () => widget,
    gardenId: () => boardFileConsentKey,
    inner: new Proxy({}, { get: () => () => { throw new Error('a board file reaches no inner bridge') } }) as ZenWidgetBridge,
    boundRows: () => [],
    ensureConversation: async () => undefined,
    push,
    stop: () => undefined,
    paused: () => false,
    pauseForRunaway: () => undefined,
    theme: () => ({ theme: { scheme: 'light', vars: {} }, chrome: { corners: null, stoplights: null }, motion: { reduced: false } }),
    onThemeChange: () => () => undefined,
    now: () => Date.now(),
    ...(boardFile ? { boardFile } : {}),
  }
  return createZenCustomLayer(deps)
}
const fakeBoardFile = (): ZenCustomBoardFile & { submits: unknown[] } => {
  const submits: unknown[] = []
  return {
    submits,
    open: () => ({ name: 'Plan', me: 'p', people: {}, meta: {}, objects: [], readOnly: null, status: { state: 'saved', text: 'Saved' } }),
    submit: (p) => submits.push(p),
    hold: () => undefined,
    flush: async () => undefined,
    key: () => undefined,
    events: () => () => undefined,
  }
}
const call = (l: ReturnType<typeof layer>, verb: string, args: unknown[] = []): Promise<{ value?: unknown; error?: { code: string } }> =>
  l.handle({ id: 1, verb, args }) as Promise<{ value?: unknown; error?: { code: string } }>

describe('the tab’s layer', () => {
  it('answers boardFile verbs from the tab', async () => {
    const b = fakeBoardFile()
    const l = layer(b)
    const open = await call(l, 'boardFile.open')
    expect(open.error).toBeUndefined()
    expect((open.value as { name: string }).name).toBe('Plan')
    const sub = await call(l, 'boardFile.submit', [{ gesture: 't', kind: 'edit', byAll: false, part: 0, last: true, text: '[]' }])
    expect(sub.error).toBeUndefined()
    expect(b.submits).toHaveLength(1)
  })

  it('takes no more than a message at a time, and refuses a bad gesture', async () => {
    const l = layer(fakeBoardFile())
    const big = await call(l, 'boardFile.submit', [{ gesture: 't', kind: 'edit', byAll: false, part: 0, last: true, text: 'x'.repeat(70_000) }])
    expect(big.error?.code).toBe('too_large')
    const bad = await call(l, 'boardFile.submit', [{ gesture: 't', kind: 'rewrite-the-file', byAll: false, part: 0, last: true, text: '[]' }])
    expect(bad.error?.code).toBe('bad_args')
  })

  it('reaches nothing else: no agents, no Thread, no Garden data, no folders', async () => {
    const l = layer(fakeBoardFile())
    for (const [verb, code] of [
      ['agents.list', 'cap_not_granted'],
      ['thread.post', 'cap_not_granted'],
      ['store.get', 'verb_unavailable'],
      ['store.batch', 'verb_unavailable'],
      ['fs.write', 'unknown_verb'],
    ] as const) {
      const r = await call(l, verb, verb.startsWith('store') ? ['k'] : [])
      expect(r.error?.code, verb).toBe(code)
    }
  })

  it('in a Garden frame every boardFile verb is unavailable, and the feature isn’t in its hello', async () => {
    const l = layer(undefined)
    for (const verb of ['boardFile.open', 'boardFile.submit', 'boardFile.hold', 'boardFile.flush', 'boardFile.key']) {
      const r = await call(l, verb, [{ gesture: 't', kind: 'edit', part: 0, last: true, text: '', key: 'k', code: 'KeyT' }])
      expect(r.error?.code, verb).toBe('verb_unavailable')
    }
    expect(ZEN_FRAME_FEATURES).not.toContain('board-file-v1')
    expect(BOARD_FILE_FEATURES).toContain('board-file-v1')
  })
})

describe('the theme', () => {
  it('maps the app’s colours onto the frame’s, and reads the scheme from them', () => {
    const el = document.createElement('div')
    document.body.appendChild(el)
    el.style.setProperty('--color-bg-canvas', '#101010')
    el.style.setProperty('--color-text-primary', '#eeeeee')
    el.style.setProperty('--color-accent', '#3366ff')
    const dark = boardFileTheme(el)
    expect(dark.scheme).toBe('dark')
    expect(dark.vars['--zen-canvas']).toBe('#101010')
    expect(dark.vars['--zen-text']).toBe('#eeeeee')
    expect(dark.vars['--zen-accent']).toBe('#3366ff')
    el.style.setProperty('--color-bg-canvas', 'rgb(250, 250, 250)')
    expect(boardFileTheme(el).scheme).toBe('light')
    el.remove()
  })
})

describe('K2’s keys', () => {
  it('hands back only K2’s own chords', () => {
    const heard: string[] = []
    const on = (e: KeyboardEvent): void => void heard.push(`${e.metaKey ? 'Meta+' : ''}${e.code}`)
    window.addEventListener('keydown', on)
    expect(redispatchBoardFileKey({ key: 't', code: 'KeyT', meta: true, ctrl: false, shift: false })).toBe(true)
    expect(redispatchBoardFileKey({ key: '2', code: 'Digit2', meta: true, ctrl: false, shift: false })).toBe(true)
    expect(redispatchBoardFileKey({ key: 'q', code: 'KeyQ', meta: true, ctrl: false, shift: false })).toBe(false)
    expect(redispatchBoardFileKey({ key: 'Enter', code: 'Enter', meta: false, ctrl: false, shift: false })).toBe(false)
    expect(redispatchBoardFileKey({ key: 't', code: 'KeyT', meta: false, ctrl: false, shift: false })).toBe(false)
    window.removeEventListener('keydown', on)
    expect(heard).toEqual(['Meta+KeyT', 'Meta+Digit2'])
  })
})

describe('a file’s parts', () => {
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3])
  function setup(blobs: NonNullable<Parameters<typeof createFakeBoardDaemon>[0]>['blobs'] = {}): { blobs: ReturnType<typeof createBoardFileBlobs>; d: ReturnType<typeof createFakeBoardDaemon>; s: ReturnType<typeof createBoardFileSession> } {
    const d = createFakeBoardDaemon({ blobs })
    const s = createBoardFileSession({ api: d, path: '/w/P.k5board', setTimer: vi.fn(), clearTimer: vi.fn(), now: () => 1, visible: () => true })
    return { blobs: createBoardFileBlobs(d, '/w/P.k5board', s), d, s }
  }

  it('a picture crosses, once, and the session keeps it for a rescue', async () => {
    const { blobs, d, s } = setup({ 'sha256-a': { mime: 'image/png', bytes: png } })
    const got = await blobs.get('k2blob:sha256-a', {})
    expect(new Uint8Array(got.bytes)).toEqual(png)
    expect(got.info.mime).toBe('image/png')
    await blobs.get('k2blob:sha256-a', {})
    expect(d.calls.filter((c) => c === 'board/file/blob')).toHaveLength(1)
    expect(s.cachedBlob('sha256-a')?.mime).toBe('image/png')
  })

  it('a part that isn’t a picture never crosses: SVG, or a manifest that lies', async () => {
    const svg = new TextEncoder().encode('<svg onload="alert(1)"/>')
    const { blobs } = setup({ 'sha256-s': { mime: 'image/png', bytes: svg, display: false } })
    await expect(blobs.get('k2blob:sha256-s', {})).rejects.toMatchObject({ wire: { code: 'blob_missing' } })
  })

  it('a new picture goes up with its hash, chunked, and the frame gets a k2blob ref', async () => {
    const { blobs, d } = setup()
    const r = await blobs.put(png.slice().buffer as ArrayBuffer, { name: 'a.png' })
    expect(r.ref).toMatch(/^k2blob:sha256-[0-9a-f]{64}$/)
    const body = d.writes[0].body as { hash: string; offset: number; total: number; mime: string; data: string }
    expect(Object.keys(body).sort()).toEqual(['data', 'hash', 'mime', 'offset', 'total'])
    expect(`k2blob:${body.hash}`).toBe(r.ref)
    expect(body.mime).toBe('image/png')
  })

  it('refuses to send up what isn’t a picture', async () => {
    const { blobs, d } = setup()
    await expect(blobs.put(new TextEncoder().encode('<svg/>').buffer as ArrayBuffer, {})).rejects.toMatchObject({ wire: { code: 'bad_args' } })
    expect(d.writes).toEqual([])
  })
})
