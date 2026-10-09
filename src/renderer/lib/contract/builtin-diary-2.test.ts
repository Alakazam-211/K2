// @vitest-environment jsdom
// The built-in Diary widget, k2:diary@2 (prd-zen-user-widgets-v2 UWB21,
// UWB24; the haunted Diary, Rosson 2026-10-08), run byte for byte against a
// fake `k2` object with a hand-cranked frame clock. @2 is @1 plus three
// asks from Rosson (2026-10-08): your sent words stay dark ink; words read
// as markdown, built as nodes; and a ghost teases in the empty pen. The
// last block of this file pins those; the rest is @1's behaviour, kept.
// DIARY_DIR points the same file at a user copy (`k2 zen widget new
// diary-lab --from k2:diary`) to check one before it is ported. The real frame, bridge
// and caps are B4's (TUW3.x); this pins the widget's own behaviour:
//   - one page per agent on THIS computer: a remote server's row never gets
//     a page, even when K2 hands it over;
//   - turning the page (keys, a corner click, a corner drag) changes who you
//     are writing to, and the Thread subscription follows the page;
//   - writing on a page posts to that page's agent only; a refusal gives the
//     words back and says why;
//   - a new reply bleeds in as handwriting; a tap shows it all;
//   - reduced motion: no turning leaf, the turn is instant, replies whole;
//   - agent text stays text: a label carrying markup never becomes an element.
// The fake `k2` keeps the real runtime's order (the 0.45.1 smoke bug): the
// widget's script runs first, `k2.can()` is false and `k2.config` /
// `k2.motion` are empty until the hello, and the hello comes after, on the
// frame's load. A widget that reads them before `await k2.connected` draws
// as if it had no access, here as in the real frame. The real runtime with
// a late hello is pinned in builtin-real-frame.test.ts.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const DIR = process.env.DIARY_DIR || resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets/diary-2')
const HTML = readFileSync(resolve(DIR, 'index.html'), 'utf8')
const JS = readFileSync(resolve(DIR, 'diary.js'), 'utf8')
const CSS = readFileSync(resolve(DIR, 'diary.css'), 'utf8')
const MANIFEST = JSON.parse(readFileSync(resolve(DIR, 'manifest.json'), 'utf8')) as { requires: { libs: string[] } }
// K2's real perfect-freehand (the vendored stdlib file the frame inlines).
const PF = new Function(
  readFileSync(resolve(__dirname, '../../zen-lib/perfect-freehand@1.2.3/perfect-freehand.js'), 'utf8') + '\nreturn PerfectFreehand',
)() as { getStroke: (p: number[][], o: object) => number[][] }

interface Row {
  address: string
  label: string
  index: number
  server: string | null
  state: string
  stateLabel: string | null
  detail: string | null
  working: boolean
  needsYou: boolean
  activity: string | null
  openable: boolean
  selected: boolean
  avatar: string | null
}

const row = (handle: string, index: number, extra: Partial<Row> = {}): Row => ({
  address: `${handle}::local`,
  label: handle.charAt(0).toUpperCase() + handle.slice(1),
  index,
  server: null,
  state: 'ok',
  stateLabel: null,
  detail: null,
  working: false,
  needsYou: false,
  activity: 'idle',
  openable: true,
  selected: false,
  avatar: null,
  ...extra,
})

/** An agent on another server: never a page. */
const remote = (handle: string, index: number): Row => ({ ...row(handle, index), address: `${handle}::box.example.test`, server: 'box.example.test' })

const item = (seq: number, body: string, mine: boolean, extra: Record<string, unknown> = {}) => ({
  seq,
  id: `m${seq}`,
  doc: { id: `m${seq}`, kind: 'message', from: mine ? 'alice' : 'agent', body, via: mine ? 'compose' : 'thread', created_at: 1_800_000_000 + seq, ...extra },
})

type Cb = (v: unknown) => void

// A macrotask: K2's hello and the promises after it settle. Under fake
// timers (the ink, paper and sound tests) the fake clock runs it.
const flush = (): Promise<unknown> => (vi.isFakeTimers() ? vi.advanceTimersByTimeAsync(0) : new Promise((r) => setTimeout(r, 0)))

/** K2's libraries the Diary may find on `window` (the real frame inlines
 *  them before the widget's script): the real perfect-freehand, and fakes
 *  for PixiJS and Tone.js (no WebGL or Web Audio in jsdom). */
interface Libs {
  PerfectFreehand?: unknown
  PIXI?: unknown
  Tone?: unknown
  IntersectionObserver?: unknown
}

async function harness(opts: { reduced?: boolean; can?: (v: string) => boolean; hello?: 'late' | 'never'; libs?: Libs; config?: Record<string, unknown> } = {}) {
  let rowsCb: Cb | null = null
  const threads = new Map<string, Cb>()
  let frames: Array<(t: number) => void> = []
  let now = 0
  let helloed = false
  let resolveHello: (v: unknown) => void = () => {}
  let rejectHello: (e: unknown) => void = () => {}
  const connected = new Promise((res, rej) => {
    resolveHello = res
    rejectHello = rej
  })
  connected.catch(() => {})
  const k2 = {
    config: opts.config ?? {},
    get motion() {
      return { reduced: helloed && opts.reduced === true }
    },
    get widget() {
      return helloed ? { id: 'diary', name: 'Diary', garden: 'g-test0001' } : null
    },
    connected,
    // Like the runtime: false for every verb until the hello.
    can: vi.fn((v: string) => helloed && (opts.can ? opts.can(v) : true)),
    ready: vi.fn(),
    agents: {
      subscribe: vi.fn((cb: Cb) => {
        rowsCb = cb
        return () => {}
      }),
    },
    thread: {
      subscribe: vi.fn((address: string, cb: Cb) => {
        threads.set(address, cb)
        return vi.fn(() => void threads.delete(address))
      }),
      read: vi.fn(async () => ({ items: [item(1, 'the very first words', false)], hasMore: false })),
      post: vi.fn(async () => ({ id: 'm99', seq: 99 })),
      answer: vi.fn(async () => null),
    },
  }
  document.documentElement.className = ''
  document.head.innerHTML = ''
  const style = document.createElement('style')
  style.textContent = CSS
  document.head.appendChild(style)
  document.body.innerHTML = HTML.slice(HTML.indexOf('<body>') + 6, HTML.indexOf('<script'))
  const winListeners: Record<string, Array<() => void>> = {}
  const win = {
    k2,
    ...opts.libs,
    devicePixelRatio: 2,
    addEventListener: (type: string, fn: () => void) => void (winListeners[type] ??= []).push(fn),
  } as unknown as Window
  const raf = (f: (t: number) => void) => {
    frames.push(f)
    return frames.length
  }
  const perf = { now: () => now }
  // The widget runs as a classic script; give it its globals explicitly.
  new Function('window', 'document', 'requestAnimationFrame', 'cancelAnimationFrame', 'performance', JS)(
    win,
    document,
    raf,
    () => {},
    perf,
  )
  // The widget's script has run; K2's hello comes now, on the frame's load.
  // Nothing may have subscribed yet: before the hello there is no access.
  expect(k2.agents.subscribe).not.toHaveBeenCalled()
  if (opts.hello === 'never') {
    rejectHello(Object.assign(new Error('not connected'), { name: 'K2Error', code: 'failed', verb: 'connected' }))
  } else {
    helloed = true
    resolveHello({ caps: ['agents:read', 'thread:read', 'thread:post'], features: [], widget: k2.widget, config: k2.config, motion: k2.motion })
  }
  await flush()
  const $ = (id: string): HTMLElement => {
    const el = document.getElementById(id)
    if (!el) throw new Error(`no #${id}`)
    return el
  }
  return {
    k2,
    rows: (r: Row[]) => (rowsCb as Cb)(r),
    /** Push a Thread view to the open page's subscription. */
    view(address: string, v: Record<string, unknown>) {
      const cb = threads.get(address)
      if (!cb) throw new Error(`no Thread subscription for ${address}; following: ${[...threads.keys()].join(', ')}`)
      cb({ address, phase: 'ready', note: null, hasMore: false, turn: null, ...v })
    },
    following: () => [...threads.keys()],
    /** An event on the frame's window (pagehide: K2 removed the frame). */
    windowEvent: (type: string) => (winListeners[type] ?? []).forEach((f) => f()),
    /** The harness's frame clock now. */
    now: () => now,
    advance(ms: number) {
      now += ms
      const run = frames
      frames = []
      run.forEach((f) => f(now))
    },
    /** Run frames until none are left (an animation finishing). */
    settle() {
      for (let i = 0; i < 200 && frames.length; i++) this.advance(16)
    },
    key: (key: string) => document.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true })),
    who: () => $('who').textContent ?? '',
    $,
  }
}

describe('k2:diary@2, the haunted Diary', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('one page per agent on this computer: a remote agent never gets a page; labels stay text; ready once', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0, { label: '<img src=x onerror=alert(1)>' }), remote('julie', 1), row('nora', 2)])
    expect(h.who()).toBe('<img src=x onerror=alert(1)>')
    expect(document.querySelector('#who img, .page img')).toBeNull()
    expect(h.$('caption').textContent).toBe('you are writing to')
    expect(h.$('folio').textContent).toBe('page i of ii')
    expect(h.following()).toEqual(['cortana::local'])
    // Every page, turned through: never Julie (box.example.test).
    const seen = [h.who()]
    for (let i = 0; i < 3; i++) {
      h.key('ArrowRight')
      seen.push(h.who())
    }
    expect([...new Set(seen)]).toEqual(['<img src=x onerror=alert(1)>', 'Nora'])
    expect(h.k2.thread.subscribe.mock.calls.map((c) => c[0])).toEqual(['cortana::local', 'nora::local'])
    expect(document.body.textContent).not.toContain('Julie')
    h.rows([row('cortana', 0), remote('julie', 1), row('nora', 2)])
    expect(h.k2.ready).toHaveBeenCalledTimes(1)
  })

  it('turning the page changes who you write to; your words go to that agent’s Thread only', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0), row('nora', 1)])
    expect((h.$('prev') as HTMLButtonElement).disabled).toBe(true)
    h.key('PageDown')
    expect(h.who()).toBe('Nora')
    expect(h.following()).toEqual(['nora::local'])
    expect(h.$('ink-label').textContent).toBe('Write to Nora')
    expect((h.$('next') as HTMLButtonElement).disabled).toBe(true)
    h.view('nora::local', { items: [] })
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.value = 'Who wrote here before me?'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.post).toHaveBeenCalledWith('nora::local', 'Who wrote here before me?')
    expect(ink.value).toBe('')
    await flush()
    // A keyboard click on the back corner turns back; the pen follows.
    h.$('prev').dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 0 }))
    expect(h.who()).toBe('Cortana')
    h.view('cortana::local', { items: [] })
    ink.value = 'And you?'
    h.$('pen').dispatchEvent(new Event('submit', { cancelable: true }))
    expect(h.k2.thread.post.mock.calls).toEqual([
      ['nora::local', 'Who wrote here before me?'],
      ['cortana::local', 'And you?'],
    ])
  })

  it('arrows don’t turn the page while you are writing; PageDown does', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0), row('nora', 1)])
    h.view('cortana::local', { items: [] })
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.value = 'half a thought'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }))
    expect(h.who()).toBe('Cortana')
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'PageDown', bubbles: true }))
    expect(h.who()).toBe('Nora')
    // The draft waits on its own page.
    h.key('PageUp')
    expect((h.$('ink') as HTMLTextAreaElement).value).toBe('half a thought')
  })

  it('with motion, a turn lifts the old page as a leaf over the new one, then follows the new Thread', async () => {
    const h = await harness()
    h.rows([row('cortana', 0), row('nora', 1)])
    h.$('next').dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 0 }))
    const leaf = document.querySelector<HTMLElement>('.leaf.next')
    expect(leaf).not.toBeNull()
    // The leaf is a copy: no ids, nothing to focus.
    expect(leaf?.querySelector('[id]')).toBeNull()
    expect(h.who()).toBe('Nora')
    expect(h.following()).toEqual(['cortana::local'])
    h.advance(300)
    expect(leaf?.style.transform).toMatch(/^rotateY\(-\d+(\.\d+)?deg\)$/)
    h.settle()
    expect(document.querySelector('.leaf')).toBeNull()
    expect(h.following()).toEqual(['nora::local'])
  })

  it('grab the corner and drag: a tiny tug lifts nothing, a real drag turns the page', async () => {
    const h = await harness()
    h.rows([row('cortana', 0), row('nora', 1)])
    const next = h.$('next')
    next.setPointerCapture = () => {}
    const pointer = (type: string, x: number): void =>
      void next.dispatchEvent(Object.assign(new MouseEvent(type, { bubbles: true, clientX: x, button: 0 }), { pointerId: 1 }))
    pointer('pointerdown', 300)
    pointer('pointermove', 297)
    expect(document.querySelector('.leaf')).toBeNull()
    pointer('pointermove', 200)
    expect(document.querySelector('.leaf.next')).not.toBeNull()
    pointer('pointerup', 200)
    h.settle()
    expect(document.querySelector('.leaf')).toBeNull()
    expect(h.who()).toBe('Nora')
    expect(h.following()).toEqual(['nora::local'])
  })

  it('history is shown whole; a new reply bleeds in as handwriting; a tap shows it all', async () => {
    const h = await harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'hello', true), item(2, 'old reply', false)] })
    expect(h.$('entries').textContent).toContain('old reply')
    expect(document.querySelector('.entry.bleeding')).toBeNull()
    const reply = 'A new reply, bleeding through the paper.'
    h.view('cortana::local', { items: [item(1, 'hello', true), item(2, 'old reply', false), item(3, 'next', true), item(4, reply, false)] })
    const body = document.querySelector('.entry.bleeding .body') as HTMLElement
    expect(body).not.toBeNull()
    h.advance(200)
    expect((body.textContent ?? '').length).toBeGreaterThan(0)
    expect((body.textContent ?? '').length).toBeLessThan(reply.length)
    // The wet edge of the ink is its own span.
    expect(body.querySelector('.wet')?.textContent?.length).toBeGreaterThan(0)
    // A later push (the turn ends) keeps the hand going, not restarting it.
    h.view('cortana::local', { items: [item(1, 'hello', true), item(2, 'old reply', false), item(3, 'next', true), item(4, reply, false)], turn: null })
    expect(document.querySelector('.entry.bleeding')).not.toBeNull()
    h.$('sheet').dispatchEvent(new MouseEvent('click', { bubbles: true }))
    expect(document.querySelector('.entry.bleeding')).toBeNull()
    expect(h.$('entries').textContent).toContain(reply)
  })

  it('a page with no history still bleeds in its first reply', async () => {
    const h = await harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    expect(h.$('note').textContent).toBe('A blank page. Write something, and see what writes back.')
    h.view('cortana::local', { items: [item(7, 'I have been waiting.', false)] })
    expect(document.querySelector('.entry.bleeding')).not.toBeNull()
  })

  it('reduced motion: the turn is instant with no leaf, and replies arrive whole', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0), row('nora', 1)])
    expect(document.documentElement.classList.contains('reduced')).toBe(true)
    h.$('next').dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 0 }))
    expect(document.querySelector('.leaf')).toBeNull()
    expect(h.who()).toBe('Nora')
    h.view('nora::local', { items: [item(1, 'hi', true)] })
    h.view('nora::local', { items: [item(1, 'hi', true), item(2, 'whole at once', false)] })
    expect(document.querySelector('.entry.bleeding')).toBeNull()
    expect(h.$('entries').textContent).toContain('whole at once')
  })

  it('sending soaks the copy in the pen into the page; the words you sent stay on the page', async () => {
    const h = await harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    const ink = h.$('ink') as HTMLTextAreaElement
    expect(h.$('pen').hidden).toBe(false)
    ink.value = 'Dear Cortana, how goes it?'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.post).toHaveBeenCalledWith('cortana::local', 'Dear Cortana, how goes it?')
    expect(h.$('soak').classList.contains('on')).toBe(true)
    expect(h.$('soak').textContent).toBe('Dear Cortana, how goes it?')
    await flush()
    h.view('cortana::local', { items: [{ ...item(99, 'Dear Cortana, how goes it?', true), id: 'm99' }] })
    const mine = document.querySelector('.entry.mine') as HTMLElement
    expect(mine.textContent).toBe('Dear Cortana, how goes it?')
  })

  it('a refused post gives the words back and says why, in the Diary’s words', async () => {
    const h = await harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    h.k2.thread.post.mockRejectedValueOnce(Object.assign(new Error('off'), { code: 'sending_off' }))
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.value = 'kept'
    h.$('pen').dispatchEvent(new Event('submit', { cancelable: true }))
    await flush()
    await flush()
    expect(ink.value).toBe('kept')
    expect(h.$('note').textContent).toBe('The ink will not take: too many words, too fast. Resume the diary to write again.')
  })

  it('working stirs the ink; needs-you lifts the bookmark, and the corner toward a calling page smoulders', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0), row('mara', 1, { needsYou: true, activity: 'needs-you' })])
    expect(h.$('next').classList.contains('calls')).toBe(true)
    expect(h.$('prev').classList.contains('calls')).toBe(false)
    h.view('cortana::local', { items: [item(1, 'hi', true)], turn: { state: 'working', since: 1 } })
    expect(h.$('stir').hidden).toBe(false)
    expect(h.$('mood').textContent).toBe('stirs…')
    expect(h.$('bookmark').hidden).toBe(true)
    h.view('cortana::local', { items: [item(1, 'hi', true)], turn: { state: 'needs-you', since: 1 } })
    expect(h.$('stir').hidden).toBe(true)
    expect(h.$('bookmark').hidden).toBe(false)
    expect(h.$('mood').textContent).toBe('is calling for you')
  })

  it('scrolling to the top of a page reads older words with beforeSeq', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(5, 'first', true), item(6, 'r1', false)], hasMore: true })
    h.$('sheet').dispatchEvent(new Event('scroll'))
    expect(h.k2.thread.read).toHaveBeenCalledWith('cortana::local', { beforeSeq: 5, limit: 50 })
    await flush()
    await flush()
    expect(h.$('entries').textContent).toContain('the very first words')
  })

  it('choice cards answer through thread.answer; secret cards stay in K2', async () => {
    const h = await harness()
    h.rows([row('cortana', 0)])
    const choice = { prompt: 'Ship it?', options: [{ label: 'Yes' }, { label: 'No' }], allow_custom: false, status: 'pending' }
    h.view('cortana::local', {
      items: [item(1, 'q', true), item(2, '', false, { kind: 'ask', choice }), item(3, '', false, { kind: 'secret' })],
    })
    const yes = [...document.querySelectorAll('.choice button')].find((b) => b.textContent === 'Yes') as HTMLButtonElement
    yes.click()
    expect(h.k2.thread.answer).toHaveBeenCalledWith('cortana::local', 'm2', 'Yes')
    expect(h.$('entries').textContent).toContain('Answer it in K2’s Thread')
  })

  it('no agents on this computer: a blank page that says so', async () => {
    const h = await harness()
    h.rows([remote('julie', 0)])
    expect(h.$('caption').textContent).toBe('the pages are blank')
    expect(h.$('note').textContent).toBe('No spirits dwell on this computer yet. Add an agent in K2, and a page will appear for it.')
    expect(h.k2.thread.subscribe).not.toHaveBeenCalled()
    expect(h.$('pen').hidden).toBe(true)
  })

  it('without agents:read it says it can’t reach your agents (never asks for a permission), and is still ready', async () => {
    const h = await harness({ can: (v) => v !== 'agents.subscribe' })
    expect(h.k2.agents.subscribe).not.toHaveBeenCalled()
    expect(h.$('caption').textContent).toBe('the diary is sealed')
    expect(h.$('note').textContent).toBe('The diary can’t reach your agents right now.')
    expect(document.body.textContent).not.toMatch(/Allow|permission/i)
    expect(h.k2.ready).toHaveBeenCalledTimes(1)
  })

  it('the hello comes after the script (the real order): the Diary waits for it, then subscribes and turns pages', async () => {
    const h = await harness({ reduced: true })
    expect(h.k2.agents.subscribe).toHaveBeenCalledTimes(1)
    h.rows([row('cortana', 0)])
    expect(h.who()).toBe('Cortana')
    expect(h.$('note').textContent).not.toContain('sealed')
    expect(h.k2.ready).toHaveBeenCalledTimes(1)
  })

  it('no hello at all: the page says it can’t reach your agents and subscribes to nothing', async () => {
    const h = await harness({ hello: 'never' })
    expect(h.k2.agents.subscribe).not.toHaveBeenCalled()
    expect(h.$('caption').textContent).toBe('the diary is sealed')
    expect(h.$('note').textContent).toBe('The diary can’t reach your agents right now.')
    expect(h.k2.ready).not.toHaveBeenCalled()
  })
})

// ── @2: dark ink, markdown, the ghost in the pen (Rosson 2026-10-08) ──

/** The ghost's lines, read from the widget's own source. */
function ghostLines(): string[] {
  const m = /var GHOST_LINES = \[([\s\S]*?)\n {2}\]/.exec(JS)
  if (!m) throw new Error('diary.js has no GHOST_LINES list')
  return [...m[1].matchAll(/'([^'\n]*)'/g)].map((x) => x[1])
}

/** A reply that uses every piece of markdown the Diary reads. */
const MARKDOWN = [
  '# A heading',
  '',
  'Some **bold**, some *italic*, some ***both***, ~~struck~~ and `inline code`.',
  'A [link](https://k2.dev/docs "title") and <https://k2.dev>.',
  '',
  '- one',
  '- two',
  '  1. nested first',
  '  2. nested second',
  '',
  '3. three',
  '4. four',
  '',
  '> a quote',
  '> with **ink**',
  '',
  '---',
  '',
  '```js',
  'const ghost = "<b>not bold</b>"',
  '  indented()',
  '```',
].join('\n')

describe('k2:diary@2: your words stay dark, and markdown', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('a sent message stays dark, whole ink, long after every fade', async () => {
    const h = await harness()
    vi.useFakeTimers()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'from long ago', true)] })
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.value = 'Do you **remember** me?'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    await vi.advanceTimersByTimeAsync(0)
    h.view('cortana::local', { items: [item(1, 'from long ago', true), { ...item(99, 'Do you **remember** me?', true), id: 'm99' }] })
    h.$('soak').dispatchEvent(new Event('animationend'))
    // Well past the soak, the reveal and every ghost timer.
    for (let i = 0; i < 120; i++) {
      await vi.advanceTimersByTimeAsync(1000)
      h.advance(500)
    }
    const mine = [...document.querySelectorAll<HTMLElement>('.entry.mine')]
    expect(mine.map((e) => e.textContent)).toEqual(['from long ago', 'Do you remember me?'])
    for (const e of mine) {
      expect(e.className).toBe('entry mine')
      const cs = getComputedStyle(e)
      expect(cs.opacity === '' || cs.opacity === '1').toBe(true)
      expect(cs.filter === '' || cs.filter === 'none').toBe(true)
      expect(cs.fontStyle === '' || cs.fontStyle === 'normal').toBe(true)
      expect(cs.color).toBe('var(--ink-mine)')
    }
    expect(mine[1].querySelector('strong')?.textContent).toBe('remember')
    // No rule anywhere fades or blurs your words.
    expect(CSS).not.toMatch(/\.entry\.mine[^{]*\{[^}]*(opacity|blur|filter)/)
  })

  it('a reply renders headings, emphasis, lists, quotes, rules, inline code, code blocks and links as elements', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, MARKDOWN, false)] })
    const body = document.querySelector('.entry.agent .body') as HTMLElement
    expect(body.querySelector('h2.md-h1')?.textContent).toBe('A heading')
    expect([...body.querySelectorAll('strong')].map((e) => e.textContent)).toEqual(['both', 'bold', 'ink'].sort((a, b) => MARKDOWN.indexOf(a) - MARKDOWN.indexOf(b)))
    expect([...body.querySelectorAll('em')].map((e) => e.textContent)).toEqual(['italic', 'both'])
    expect(body.querySelector('strong > em')?.textContent).toBe('both')
    expect(body.querySelector('del')?.textContent).toBe('struck')
    expect(body.querySelector('p code.md-code')?.textContent).toBe('inline code')
    const ul = body.querySelector('ul.md-list') as HTMLElement
    expect([...ul.children].map((li) => li.firstElementChild?.textContent)).toEqual(['one', 'two'])
    const nested = ul.querySelector('li ol') as HTMLElement
    expect([...nested.children].map((li) => li.textContent)).toEqual(['nested first', 'nested second'])
    const ol = body.querySelector(':scope > ol.md-list') as HTMLElement
    expect(ol.getAttribute('start')).toBe('3')
    expect([...ol.children].map((li) => li.textContent)).toEqual(['three', 'four'])
    const quote = body.querySelector('blockquote.md-quote') as HTMLElement
    expect(quote.textContent).toBe('a quotewith ink')
    expect(quote.querySelector('br')).not.toBeNull()
    expect(quote.querySelector('strong')?.textContent).toBe('ink')
    expect(body.querySelector('hr.md-rule')).not.toBeNull()
    const pre = body.querySelector('pre.md-pre') as HTMLElement
    expect(pre.dataset.lang).toBe('js')
    expect(pre.querySelector('code')?.textContent).toBe('const ghost = "<b>not bold</b>"\n  indented()')
    expect(pre.querySelector('b')).toBeNull()
    // Links show their words and their address; nothing to click.
    expect(body.querySelector('.md-link')?.textContent).toBe('link')
    expect([...body.querySelectorAll('.md-url')].map((e) => e.textContent)).toEqual([' (https://k2.dev/docs)', 'https://k2.dev'])
    expect(body.querySelector('a, [href]')).toBeNull()
  })

  it('your own words read as markdown too, and plain words keep their line breaks', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'Line one\nline _two_\n\n- a\n- b', true), item(2, 'snake_case_name and 2 * 3 * 4 stay as written', false)] })
    const mine = document.querySelector('.entry.mine .body') as HTMLElement
    expect(mine.querySelector('p')?.querySelectorAll('br').length).toBe(1)
    expect(mine.querySelector('em')?.textContent).toBe('two')
    expect(mine.querySelectorAll('li').length).toBe(2)
    const agent = document.querySelector('.entry.agent .body') as HTMLElement
    expect(agent.textContent).toBe('snake_case_name and 2 * 3 * 4 stay as written')
    expect(agent.querySelector('em, strong')).toBeNull()
  })

  it('markup in a message is only ink: no script, no handler, no javascript: link', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0)])
    const evil = [
      '<script>window.pwned = 1</script>',
      '<img src=x onerror="window.pwned = 2">',
      '[click me](javascript:window.pwned=3) and <javascript:alert(1)>',
      '**<svg onload=alert(1)>**',
      '`<iframe src=x>`',
    ].join('\n\n')
    h.view('cortana::local', { items: [item(1, evil, false), item(2, evil, true)] })
    const entries = h.$('entries')
    expect(entries.querySelector('script, img, svg, iframe, a, [href], [src]')).toBeNull()
    for (const el of entries.querySelectorAll('*')) {
      for (const attr of el.getAttributeNames()) expect(attr.startsWith('on')).toBe(false)
    }
    const text = entries.textContent ?? ''
    expect(text).toContain('<script>window.pwned = 1</script>')
    expect(text).toContain('<img src=x onerror="window.pwned = 2">')
    expect(text).toContain('click me (javascript:window.pwned=3)')
    expect(text).toContain('<svg onload=alert(1)>')
    expect(text).toContain('<iframe src=x>')
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined()
  })

  it('a markdown reply bleeds in letter by letter, blocks appearing as the hand reaches them, and lands whole', async () => {
    const h = await harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'hi', true)] })
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, MARKDOWN, false)] })
    const entry = document.querySelector('.entry.bleeding') as HTMLElement
    expect(entry).not.toBeNull()
    h.advance(300)
    const body = entry.querySelector('.body') as HTMLElement
    expect([...body.querySelectorAll('.wet')].some((w) => (w.textContent ?? '').length > 0)).toBe(true)
    expect(body.querySelector('pre')?.classList.contains('unwritten')).toBe(true)
    expect(body.querySelector('h2')?.classList.contains('unwritten')).toBe(false)
    h.advance(6000) // the hand's cap
    h.settle()
    expect(document.querySelector('.entry.bleeding')).toBeNull()
    expect(body.querySelector('.wet, .unwritten')).toBeNull()
    expect(body.querySelector('pre code')?.textContent).toBe('const ghost = "<b>not bold</b>"\n  indented()')
    expect(body.querySelectorAll('li').length).toBe(6)
  })

  it('a huge run of lone delimiters renders quickly as plain ink', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0)])
    const junk = '*a _b ~c [d ![e **f '.repeat(4000)
    const t = Date.now()
    h.view('cortana::local', { items: [item(1, junk, false)] })
    expect(Date.now() - t).toBeLessThan(2000)
    expect(document.querySelector('.entry.agent')?.textContent).toBe(junk.trim())
  })
})

describe('k2:diary@2: the ghost in the empty pen', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  async function haunted(opts: { reduced?: boolean } = {}) {
    const h = await harness(opts)
    vi.useFakeTimers()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    const ink = h.$('ink') as HTMLTextAreaElement
    const ghost = h.$('ghost')
    return { h, ink, ghost, tick: (ms: number) => vi.advanceTimersByTime(ms) }
  }

  it('never a plain “write here”: no placeholder, no hint; the textarea keeps a name for screen readers', async () => {
    const { ink, ghost, h } = await haunted()
    expect(ink.getAttribute('placeholder')).toBeNull()
    expect(document.getElementById('hint')).toBeNull()
    expect(h.$('pen').textContent).not.toMatch(/write here|write a message|enter, and/i)
    expect(HTML).not.toMatch(/placeholder=/)
    expect(ink.getAttribute('aria-label')).toBe('Write to Cortana in the diary')
    expect(ghost.hidden).toBe(false)
    expect(ghost.getAttribute('aria-hidden')).toBe('true')
  })

  it('lines never ask for anything real and sensitive (a guard rail)', () => {
    const lines = ghostLines()
    expect(lines.length).toBeGreaterThanOrEqual(15)
    expect(lines.length).toBeLessThanOrEqual(20)
    expect(new Set(lines).size).toBe(lines.length)
    const words = /\b(pins?|cards?|codes?|ssn|cvv|cvc|otp|iban|keys?|banks?|tokens?|accounts?)\b/i
    const parts = /password|passcode|passphrase|login|log in|sign in|signin|credential|credit|debit|social security|routing|address|email|e-mail|phone|birthday|date of birth|maiden|security question|username|user name|wallet|seed phrase|recovery|verification|zip|postcode|license|passport/i
    for (const l of lines) {
      expect(l, l).not.toMatch(words)
      expect(l, l).not.toMatch(parts)
    }
  })

  it('writes a tease letter by letter, lingers, unwrites it, then writes another; never into the textarea', async () => {
    const { ink, ghost, tick, h } = await haunted()
    // From the first moment: no "write here", only the ghost.
    expect(ghost.hidden).toBe(false)
    expect(ink.placeholder).toBe('')
    const lines = ghostLines()
    const seen: string[] = []
    let grew = false
    let shrank = false
    let last = ''
    for (let i = 0; i < 600; i++) {
      tick(50)
      const now = ghost.textContent ?? ''
      if (now.length > last.length) grew = true
      if (now.length < last.length) shrank = true
      if (lines.includes(now) && seen[seen.length - 1] !== now) seen.push(now)
      expect(ink.value).toBe('')
      last = now
    }
    expect(grew && shrank).toBe(true)
    expect(seen.length).toBeGreaterThanOrEqual(2)
    for (const s of seen) expect(lines).toContain(s)
    // Each letter is its own unsteady glyph.
    expect(ghost.querySelectorAll('.gl').length).toBe((ghost.textContent ?? '').length)
    // An empty pen sends nothing, ghost or not.
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.post).not.toHaveBeenCalled()
  })

  it('focus freezes its line (still there, faint); the first letter sends it away; it returns after the pen is empty and left 3.5 s', async () => {
    const { ink, ghost, tick } = await haunted()
    tick(3600)
    tick(1500)
    expect(ghost.hidden).toBe(false)
    ink.focus()
    expect(ghost.hidden).toBe(false)
    expect(ghost.classList.contains('frozen')).toBe(true)
    const held = ghost.textContent
    expect(ghostLines()).toContain(held)
    tick(30_000)
    expect(ghost.textContent).toBe(held)
    ink.value = 'I'
    ink.dispatchEvent(new Event('input'))
    expect(ghost.hidden).toBe(true)
    expect(ink.placeholder).toBe('')
    ink.value = ''
    ink.dispatchEvent(new Event('input'))
    tick(30_000)
    expect(ghost.hidden).toBe(true)
    ink.blur()
    tick(3400)
    expect(ghost.hidden).toBe(true)
    tick(200)
    expect(ghost.hidden).toBe(false)
    expect(ghost.classList.contains('frozen')).toBe(false)
    // A frozen ghost left alone moves again after the pen is blurred 3.5 s.
    tick(4000)
    ink.focus()
    const again = ghost.textContent
    ink.blur()
    tick(3000)
    expect(ghost.textContent).toBe(again)
    let moved = false
    for (let i = 0; i < 100; i++) {
      tick(100)
      if (ghost.textContent !== again) moved = true
    }
    expect(moved).toBe(true)
  })

  it('selecting the pen makes nothing on the page disappear', async () => {
    const { ink, tick, h } = await haunted()
    h.view('cortana::local', { items: [item(1, 'what I sent', true), item(2, 'what it said back', false)] })
    const entries = h.$('entries')
    const before = [...entries.children]
    const text = entries.textContent
    tick(5000)
    ink.focus()
    ink.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    tick(10_000)
    expect([...entries.children]).toEqual(before)
    expect(entries.textContent).toBe(text)
    for (const e of before) expect(e.isConnected).toBe(true)
    expect(h.$('who').textContent).toBe('Cortana')
    for (const id of ['desk', 'book', 'page']) expect(h.$(id).scrollTop).toBe(0)
  })

  it('after your words sink in, it wakes at once, even in the focused pen; your next word sends it away', async () => {
    const { ink, ghost, tick, h } = await haunted()
    ink.focus()
    ink.value = 'Who are you?'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(ghost.hidden).toBe(true)
    await vi.advanceTimersByTimeAsync(0)
    h.view('cortana::local', { items: [{ ...item(99, 'Who are you?', true), id: 'm99' }] })
    expect(ghost.hidden).toBe(true)
    h.$('soak').dispatchEvent(new Event('animationend'))
    expect(ghost.hidden).toBe(false)
    tick(3000)
    expect((ghost.textContent ?? '').length).toBeGreaterThan(0)
    ink.value = 'n'
    ink.dispatchEvent(new Event('input'))
    expect(ghost.hidden).toBe(true)
  })

  it('pauses while the frame is hidden', async () => {
    const { ghost, tick } = await haunted()
    tick(3600)
    expect(ghost.hidden).toBe(false)
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true })
    document.dispatchEvent(new Event('visibilitychange'))
    expect(ghost.hidden).toBe(true)
    tick(30_000)
    expect(ghost.hidden).toBe(true)
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
    document.dispatchEvent(new Event('visibilitychange'))
    tick(3600)
    expect(ghost.hidden).toBe(false)
  })

  it('reduced motion: one still line, no writing and unwriting', async () => {
    const { ink, ghost, tick } = await haunted({ reduced: true })
    tick(3600)
    expect(ghost.hidden).toBe(false)
    expect(ghost.classList.contains('still')).toBe(true)
    const still = ghost.textContent
    expect(ghostLines()).toContain(still)
    for (let i = 0; i < 100; i++) {
      tick(100)
      expect(ghost.textContent).toBe(still)
    }
    expect(ink.value).toBe('')
    expect(CSS).toMatch(/:root\.reduced \.ghost[^{]*\{[^}]*animation: none/)
  })
})

describe('k2:diary@2: the pen scratches while the agent works', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  const paths = () => [...document.querySelectorAll<SVGPathElement>('#scribble path')]

  it('scribbles while working, a little differently each fit, and clears the instant the reply starts writing', async () => {
    const h = await harness()
    vi.useFakeTimers()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'hi', true)], turn: { state: 'working', since: 1 } })
    expect(h.$('stir').hidden).toBe(false)
    expect(document.querySelector('#stir span:not(.ghosts)')).toBeNull()
    let most = 0
    let dashed = 0
    const shapes = new Set<string>()
    for (let i = 0; i < 300; i++) {
      vi.advanceTimersByTime(50)
      most = Math.max(most, paths().length)
      for (const p of paths()) {
        shapes.add(p.getAttribute('d') ?? '')
        expect(p.namespaceURI).toBe('http://www.w3.org/2000/svg')
        // Each stroke is drawn by a dash offset (between fits the drawing
        // is briefly empty, so this is read while it's there).
        if (Number(p.style.strokeDasharray) > 0) dashed++
      }
    }
    expect(most).toBeGreaterThanOrEqual(4)
    expect(shapes.size).toBeGreaterThan(8)
    expect(dashed).toBeGreaterThan(20)
    // The reply arrives while the turn is still marked working.
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'At last, an answer.', false)], turn: { state: 'working', since: 1 } })
    expect(document.querySelector('.entry.bleeding')).not.toBeNull()
    expect(h.$('stir').hidden).toBe(true)
    expect(paths()).toEqual([])
    vi.advanceTimersByTime(5000)
    expect(paths()).toEqual([])
    // Done: no scribble.
    h.advance(7000)
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'At last, an answer.', false)], turn: null })
    expect(h.$('stir').hidden).toBe(true)
    expect(paths()).toEqual([])
  })

  it('every stroke stays inside the drawing, with room for the pen and its shadow, whatever the dice say', async () => {
    const PAD = 4
    let checked = 0
    let inks = 0
    const seen = new Set<string>()
    for (let seed = 1; seed <= 20; seed++) {
      let a = seed * 0x9e3779b9
      vi.spyOn(Math, 'random').mockImplementation(() => {
        a = (a + 0x6d2b79f5) | 0
        let t = Math.imul(a ^ (a >>> 15), 1 | a)
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296
      })
      document.body.innerHTML = ''
      const h = await harness({ libs: { PerfectFreehand: PF } })
      vi.useFakeTimers()
      h.rows([row('cortana', 0)])
      h.view('cortana::local', { items: [], turn: { state: 'working', since: 1 } })
      const svg = h.$('scribble')
      expect(svg.getAttribute('viewBox')).toBe('0 0 260 44')
      const [, , w, hgt] = (svg.getAttribute('viewBox') ?? '').split(' ').map(Number)
      for (let i = 0; i < 80; i++) {
        vi.advanceTimersByTime(250)
        for (const p of paths()) {
          const d = p.getAttribute('d') ?? ''
          if (seen.has(d)) continue
          seen.add(d)
          if (p.classList.contains('ink')) {
            inks++
            expect(p.classList.contains('line')).toBe(false)
            expect(d).toMatch(/^M[\d. ]+Q[\d. ]+T[\d. ]+Z$/)
            expect(p.getAttribute('mask')).toMatch(/^url\(#scribe-hand-\d+\)$/)
          }
          const n = d.match(/-?\d+(\.\d+)?/g)?.map(Number) ?? []
          expect(n.length % 2).toBe(0)
          for (let k = 0; k < n.length; k += 2) {
            expect(n[k]).toBeGreaterThanOrEqual(PAD)
            expect(n[k]).toBeLessThanOrEqual(w - PAD)
            expect(n[k + 1]).toBeGreaterThanOrEqual(PAD)
            expect(n[k + 1]).toBeLessThanOrEqual(hgt - PAD)
            checked++
          }
        }
      }
      vi.useRealTimers()
      vi.restoreAllMocks()
    }
    expect(seen.size).toBeGreaterThan(200)
    expect(checked).toBeGreaterThan(40_000)
    // The real pen drew them: filled outlines, each traced by a mask line.
    expect(inks).toBeGreaterThan(100)
    // The box around it has real padding, so the drawing never meets the clip.
    expect(CSS).toMatch(/\.stir \{[^}]*padding: 4px 6px 10px 6px/)
  }, 60_000)

  // Rosson 2026-10-08: "the words just ended early". A scrawled word runs
  // its full length and ends on purpose: a flick up as the pen lifts, or a
  // jab down given up on (then a stab of ink); each fit's words span the
  // line, left margin to right.
  it('every scrawled word finishes (a lifting flick, or a jab and a stab), and the words span the line', async () => {
    const W = 260
    const BASE = 27
    const nums = (d: string) => d.match(/-?\d+(\.\d+)?/g)?.map(Number) ?? []
    let fits = 0
    let words = 0
    let flicks = 0
    let jabs = 0
    for (let seed = 1; seed <= 12; seed++) {
      let a = seed * 0x2545f491
      vi.spyOn(Math, 'random').mockImplementation(() => {
        a = (a + 0x6d2b79f5) | 0
        let t = Math.imul(a ^ (a >>> 15), 1 | a)
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296
      })
      document.body.innerHTML = ''
      const h = await harness({ libs: { PerfectFreehand: PF } })
      vi.useFakeTimers()
      h.rows([row('cortana', 0)])
      h.view('cortana::local', { items: [], turn: { state: 'working', since: 1 } })
      let fit = new Map<string, { x0: number; x1: number; endY: number; jab: boolean }>()
      let stabs = 0
      const close = () => {
        if (!fit.size) return
        const ws = [...fit.values()]
        expect(Math.min(...ws.map((w) => w.x0))).toBeLessThan(W * 0.1)
        expect(Math.max(...ws.map((w) => w.x1))).toBeGreaterThan(W * 0.85)
        expect(stabs).toBeGreaterThanOrEqual(ws.filter((w) => w.jab).length)
        fits++
        fit = new Map()
        stabs = 0
      }
      for (let i = 0; i < 120; i++) {
        vi.advanceTimersByTime(200)
        const groups = [...document.querySelectorAll('#scribble g')]
        if (!groups.length) close()
        for (const g of groups) {
          const hand = g.querySelector('path.hand')?.getAttribute('d') ?? ''
          if (g.querySelector('path.ink.stab') && !fit.has('stab:' + hand)) {
            fit.set('stab:' + hand, { x0: Infinity, x1: -Infinity, endY: 0, jab: false })
            stabs++
          }
          if (!g.querySelector('path.ink.scrawl') || fit.has(hand)) continue
          const n = nums(hand)
          const xs = n.filter((_, k) => k % 2 === 0)
          const endY = n[n.length - 1]
          // It ends off the baseline, never mid-letter on it: up (a flick)
          // or down (a jab).
          const jab = endY > BASE + 1.5
          expect(jab || endY < BASE - 1).toBe(true)
          if (jab) jabs++
          else flicks++
          words++
          fit.set(hand, { x0: Math.min(...xs), x1: Math.max(...xs), endY, jab })
        }
      }
      for (const k of [...fit.keys()]) if (k.startsWith('stab:')) fit.delete(k)
      vi.useRealTimers()
      vi.restoreAllMocks()
    }
    expect(fits).toBeGreaterThan(12)
    expect(words).toBeGreaterThan(30)
    expect(flicks).toBeGreaterThan(0)
    expect(jabs).toBeGreaterThan(0)
  }, 60_000)

  it('pauses while the frame is hidden', async () => {
    const h = await harness()
    vi.useFakeTimers()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [], turn: { state: 'working', since: 1 } })
    vi.advanceTimersByTime(1500)
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true })
    document.dispatchEvent(new Event('visibilitychange'))
    expect(h.$('scribble').classList.contains('paused')).toBe(true)
    const n = paths().length
    vi.advanceTimersByTime(20_000)
    expect(paths().length).toBe(n)
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
    document.dispatchEvent(new Event('visibilitychange'))
    expect(h.$('scribble').classList.contains('paused')).toBe(false)
  })

  it('reduced motion: one still mark', async () => {
    const h = await harness({ reduced: true })
    vi.useFakeTimers()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [], turn: { state: 'working', since: 1 } })
    expect(h.$('scribble').classList.contains('still')).toBe(true)
    const marks = paths().map((p) => p.getAttribute('d'))
    expect(marks.length).toBeGreaterThan(0)
    vi.advanceTimersByTime(20_000)
    expect(paths().map((p) => p.getAttribute('d'))).toEqual(marks)
    expect(paths().every((p) => p.style.strokeDasharray === '')).toBe(true)
  })
})

describe('k2:diary@2: the page glides up as the hand writes', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  /** jsdom has no layout: give the sheet a fake one we control. */
  function scroller(sheet: HTMLElement) {
    const s = { height: 300, client: 300, top: 0, writes: 0 }
    Object.defineProperty(sheet, 'scrollHeight', { configurable: true, get: () => s.height })
    Object.defineProperty(sheet, 'clientHeight', { configurable: true, get: () => s.client })
    Object.defineProperty(sheet, 'scrollTop', {
      configurable: true,
      get: () => s.top,
      set: (v: number) => {
        s.writes++
        s.top = Math.max(0, Math.min(v, s.height - s.client))
      },
    })
    return {
      s,
      /** The reader's hand. */
      user(top: number) {
        s.top = top
        sheet.dispatchEvent(new Event('scroll'))
      },
    }
  }

  it('glides toward the bottom without steps or overshoot, and keeps up while a reply is written', async () => {
    const h = await harness()
    const f = scroller(h.$('sheet'))
    h.rows([row('cortana', 0)])
    f.s.height = 900
    h.view('cortana::local', { items: [item(1, 'hi', true)] })
    expect(f.s.top).toBe(600) // a new page opens at the bottom
    f.s.height = 1400
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'A long reply, written slowly into the page.', false)] })
    const tops: number[] = []
    for (let i = 0; i < 60; i++) {
      h.advance(16)
      tops.push(f.s.top)
    }
    const target = 1100
    for (let i = 1; i < tops.length; i++) {
      expect(tops[i]).toBeGreaterThanOrEqual(tops[i - 1])
      expect(tops[i]).toBeLessThanOrEqual(target)
      expect(tops[i] - tops[i - 1]).toBeLessThan(200) // a glide, not a jump
    }
    expect(tops[0]).toBeLessThan(target)
    expect(target - tops[tops.length - 1]).toBeLessThan(1)
    // The text keeps growing while the hand writes: the page keeps up.
    f.s.height = 1500
    for (let i = 0; i < 40; i++) h.advance(16)
    expect(f.s.top).toBeGreaterThan(1199)
    expect(f.s.top).toBeLessThanOrEqual(1200)
  })

  it('scrolling up by hand lets go; coming back to the bottom takes hold again', async () => {
    const h = await harness()
    const f = scroller(h.$('sheet'))
    h.rows([row('cortana', 0)])
    f.s.height = 900
    h.view('cortana::local', { items: [item(1, 'hi', true)] })
    f.s.height = 1400
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'A reply.', false)] })
    h.advance(16)
    h.advance(16)
    f.user(200)
    for (let i = 0; i < 60; i++) h.advance(16)
    expect(f.s.top).toBe(200)
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'A reply.', false), item(3, 'and more', false)] })
    for (let i = 0; i < 60; i++) h.advance(16)
    expect(f.s.top).toBe(200)
    f.user(1080) // back near the bottom
    f.s.height = 1700
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'A reply.', false), item(3, 'and more', false), item(4, 'still more', false)] })
    for (let i = 0; i < 80; i++) h.advance(16)
    expect(1400 - f.s.top).toBeLessThan(1)
  })

  it('reduced motion: it snaps, with no frames', async () => {
    const h = await harness({ reduced: true })
    const f = scroller(h.$('sheet'))
    h.rows([row('cortana', 0)])
    f.s.height = 900
    h.view('cortana::local', { items: [item(1, 'hi', true)] })
    f.s.height = 1300
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'whole at once', false)] })
    expect(f.s.top).toBe(1000)
  })

  it('no seal button and no whispers switch: Enter seals; the only other control is the music’s speaker, which says why it’s silent', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    expect(document.getElementById('send')).toBeNull()
    expect(document.getElementById('whispers')).toBeNull()
    expect([...document.querySelectorAll('button')].map((b) => b.id)).toEqual(['hush', 'prev', 'next'])
    // No Tone.js here: the speaker still shows, crossed out, and says why.
    expect(h.$('hush').hidden).toBe(false)
    expect(h.$('hush').classList.contains('off')).toBe(true)
    expect(h.$('hush').getAttribute('title')).toBe('Play the music (M). Music: the music library didn’t load')
    expect(JS).not.toMatch(/AudioContext|whispers|setSound/)
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.value = 'sealed with Enter'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.post).toHaveBeenCalledWith('cortana::local', 'sealed with Enter')
  })
})

describe('k2:diary@2: how many ghosts are out (0.45.2 counts)', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  async function working(counts: unknown, present = true) {
    const h = await harness({ reduced: true })
    const r = row('cortana', 0, { working: true, activity: 'working' })
    h.rows([present ? ({ ...r, counts } as Row) : r])
    h.view('cortana::local', { items: [item(1, 'hi', true)], turn: { state: 'working', since: 1 } })
    expect(h.$('stir').hidden).toBe(false)
    return { h, line: h.$('ghosts') }
  }

  it('no counts from K2 (0.45.1), null counts, or none out: nothing extra, no made-up number', async () => {
    for (const [counts, present] of [[undefined, false], [null, true], [{ subagents: 0, tools: 9, commands: 2 }, true], [{ tools: 3 }, true], [{ subagents: 'three' }, true]] as const) {
      document.body.innerHTML = ''
      const { line } = await working(counts, present)
      expect(line.hidden).toBe(true)
      expect(line.textContent).toBe('')
    }
  })

  it('one ghost: a singular line', async () => {
    const { line } = await working({ subagents: 1, tools: 2, commands: 0 })
    expect(line.hidden).toBe(false)
    expect(line.textContent).toMatch(/\b(a|one|lone) ghost\b/)
    expect(line.textContent).not.toMatch(/ghosts/)
  })

  it('three ghosts: a plural line, the number in words; many tools rattle chains', async () => {
    const { line, h } = await working({ subagents: 3, tools: 2, commands: 1 })
    expect(line.textContent).toMatch(/\bthree ghosts\b/)
    expect(line.textContent).not.toMatch(/chains/)
    h.rows([{ ...row('cortana', 0, { working: true, activity: 'working' }), counts: { subagents: 7, tools: 14, commands: 3 } } as Row])
    expect(line.textContent).toMatch(/\bseven ghosts\b/)
    expect(line.textContent).toMatch(/rattling 14 chains$/)
    // The line goes with the scribble when the reply starts writing.
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'done', false)], turn: null })
    h.rows([{ ...row('cortana', 0), counts: { subagents: 0, tools: 0, commands: 0 } } as Row])
    expect(h.$('stir').hidden).toBe(true)
    expect(line.hidden).toBe(true)
  })
})

// ── ink, paper and sound (Rosson 2026-10-08) ─────────────────────────────
// perfect-freehand for the ink, PixiJS for the paper under the words, and
// Tone.js for the room's music, each from K2's library. jsdom has neither
// WebGL nor Web Audio, so PixiJS and Tone.js are fakes that record what the
// Diary asks of them; perfect-freehand is the real vendored file.

/** A 2d context good enough for the paper tile (jsdom has none). */
function fake2d(): void {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    () =>
      ({
        createImageData: (w: number, hgt: number) => ({ data: new Uint8ClampedArray(w * hgt * 4) }),
        putImageData: () => {},
        beginPath: () => {},
        moveTo: () => {},
        quadraticCurveTo: () => {},
        stroke: () => {},
      }) as unknown as CanvasRenderingContext2D,
  )
}

function fakePixi(opts: { fail?: boolean } = {}) {
  const log = {
    inits: [] as Array<Record<string, unknown>>,
    renders: 0,
    shader: null as null | { gl: { vertex: string; fragment: string } },
    group: null as null | { uniforms: Record<string, Float32Array> },
    tickerStarts: 0,
    resized: [] as number[][],
    destroyed: 0,
  }
  class UniformGroup {
    uniforms: Record<string, Float32Array>
    updates = 0
    constructor(u: Record<string, { value: Float32Array }>) {
      this.uniforms = Object.fromEntries(Object.entries(u).map(([k, v]) => [k, v.value]))
      log.group = this
    }
    update() {
      this.updates++
    }
  }
  class Application {
    stage = { addChild: () => {} }
    renderer = { resize: (w: number, hgt: number) => void log.resized.push([w, hgt]) }
    ticker = { start: () => void log.tickerStarts++, stop: () => {}, add: () => {} }
    init(o: Record<string, unknown>) {
      log.inits.push(o)
      return opts.fail ? Promise.reject(new Error('no WebGL')) : Promise.resolve()
    }
    render() {
      log.renders++
    }
    destroy() {
      log.destroyed++
    }
  }
  const PIXI = {
    Application,
    UniformGroup,
    Texture: { from: () => ({ source: {} as Record<string, unknown> }) },
    Shader: { from: (o: { gl: { vertex: string; fragment: string } }) => ((log.shader = o), {}) },
    Geometry: class {},
    Mesh: class {},
  }
  return { PIXI, log }
}

function fakeTone(opts: { blocked?: boolean } = {}) {
  const log = {
    contexts: [] as Array<Record<string, unknown>>,
    resumes: 0,
    suspends: 0,
    closes: 0,
    master: [] as Array<[number, number]>, // the master gain's ramps: [to, seconds]
    nodes: 0,
    triggers: 0,
    allowed: !opts.blocked, // a gesture lets a held-back context run
    gains: [] as Array<Array<[number, number, number]>>, // per Gain node: [to, seconds, at ms]
  }
  let first: Node | null = null
  class Param {
    value = 0
    constructor(private owner: Node) {}
    rampTo(v: number, t: number) {
      if (this.owner === first) log.master.push([v, t])
      if (this.owner.ramps) this.owner.ramps.push([v, t, Date.now()])
      this.value = v
      return this
    }
    setValueAtTime() {
      return this
    }
    linearRampToValueAtTime() {
      return this
    }
  }
  class Node {
    ramps: Array<[number, number, number]> | null = null
    gain = new Param(this)
    frequency = new Param(this)
    pan = new Param(this)
    constructor() {
      log.nodes++
      if (!first) first = this // the master gain is the first node made
    }
    connect() {
      return this
    }
    toDestination() {
      return this
    }
    start() {
      return this
    }
    triggerAttackRelease() {
      log.triggers++
      return this
    }
  }
  class Context {
    state = 'suspended'
    updateInterval = 0
    rawContext: { suspend: () => Promise<void> }
    constructor(o: Record<string, unknown>) {
      log.contexts.push(o)
      this.rawContext = {
        suspend: () => {
          log.suspends++
          this.state = 'suspended'
          return Promise.resolve()
        },
      }
    }
    resume() {
      log.resumes++
      if (!log.allowed) return new Promise<void>(() => {}) // held back: never settles
      this.state = 'running'
      return Promise.resolve()
    }
    close() {
      log.closes++
      this.state = 'closed'
    }
  }
  class Gain extends Node {
    constructor() {
      super()
      this.ramps = []
      log.gains.push(this.ramps)
    }
  }
  const Tone = {
    Context,
    setContext: () => {},
    now: () => 0,
    Gain,
    Reverb: Node,
    FeedbackDelay: Node,
    Filter: Node,
    Oscillator: Node,
    LFO: Node,
    Noise: Node,
    PolySynth: Node,
    FMSynth: Node,
    Panner: Node,
    AmplitudeEnvelope: Node,
  }
  return { Tone, log }
}

const hide = (hidden: boolean) => {
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('k2:diary@2: ink from perfect-freehand', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('the manifest asks K2 for its libraries by major version', () => {
    expect(MANIFEST.requires.libs).toEqual(['perfect-freehand@1', 'font-caveat@5', 'pixi.js@8', 'tone@15'])
  })

  it('the flourish under a name is a filled, tapered outline inside its box', async () => {
    const h = await harness({ reduced: true, libs: { PerfectFreehand: PF } })
    h.rows([row('cortana', 0), row('nora', 1)])
    for (let page = 0; page < 2; page++) {
      const d = h.$('flourish').getAttribute('d') ?? ''
      expect(d).toMatch(/^M[\d. ]+Q[\d. ]+T[\d. ]+Z$/)
      const n = d.match(/-?\d+(\.\d+)?/g)?.map(Number) ?? []
      for (let k = 0; k < n.length; k += 2) {
        expect(n[k]).toBeGreaterThanOrEqual(0.5)
        expect(n[k]).toBeLessThanOrEqual(159.5)
        expect(n[k + 1]).toBeGreaterThanOrEqual(0.5)
        expect(n[k + 1]).toBeLessThanOrEqual(15.5)
      }
      h.key('ArrowRight')
    }
  })

  it('some replies get a blot where the pen rested: the same ones every time, inside their box, never on your words', async () => {
    const h = await harness({ reduced: true, libs: { PerfectFreehand: PF } })
    h.rows([row('cortana', 0)])
    const items = Array.from({ length: 40 }, (_, i) => item(i + 1, `line ${i + 1}`, i % 5 === 0))
    h.view('cortana::local', { items })
    const blotted = () => [...document.querySelectorAll('.entry')].filter((e) => e.querySelector('.blot')).map((e) => (e as HTMLElement).dataset.id)
    const first = blotted()
    expect(first.length).toBeGreaterThan(3)
    expect(first.length).toBeLessThan(25)
    expect(document.querySelectorAll('.entry.mine .blot').length).toBe(0)
    for (const svg of document.querySelectorAll('.blot')) {
      expect(svg.getAttribute('aria-hidden')).toBe('true')
      for (const p of svg.querySelectorAll('path')) {
        const n = (p.getAttribute('d') ?? '').match(/-?\d+(\.\d+)?/g)?.map(Number) ?? []
        for (let k = 0; k < n.length; k += 2) {
          expect(n[k]).toBeGreaterThanOrEqual(0.5)
          expect(n[k]).toBeLessThanOrEqual(39.5)
          expect(n[k + 1]).toBeGreaterThanOrEqual(0.5)
          expect(n[k + 1]).toBeLessThanOrEqual(27.5)
        }
      }
    }
    h.view('cortana::local', { items })
    expect(blotted()).toEqual(first)
    // A blot waits for the words to dry.
    expect(CSS).toMatch(/\.entry\.bleeding \.blot \{ opacity: 0;/)
  })

  it('without the library: plain lines, no blots, nothing breaks', async () => {
    const h = await harness()
    vi.useFakeTimers()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: Array.from({ length: 12 }, (_, i) => item(i + 1, 'x', false)), turn: { state: 'working', since: 1 } })
    vi.advanceTimersByTime(1500)
    expect(document.querySelectorAll('.blot').length).toBe(0)
    expect(document.querySelectorAll('#scribble path.ink.line').length).toBeGreaterThan(0)
  })
})

describe('k2:diary@2: the paper under the words (PixiJS)', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
    fake2d()
  })
  afterEach(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  /** Move both clocks: K2's timers and the frame clock, a frame at a time. */
  const run = (h: Awaited<ReturnType<typeof harness>>, ms: number) => {
    for (let t = 0; t < ms; t += 16) {
      vi.advanceTimersByTime(16)
      h.advance(16)
    }
  }

  it('one WebGL canvas, first in the page and under every word; no ticker of its own', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    const canvas = document.querySelector('canvas.paperfx')
    expect(canvas).not.toBeNull()
    expect(h.$('page').firstElementChild).toBe(canvas)
    expect(canvas?.getAttribute('aria-hidden')).toBe('true')
    expect(document.documentElement.classList.contains('fx')).toBe(true)
    expect(log.inits[0]).toMatchObject({ canvas, backgroundAlpha: 0, preference: 'webgl', autoStart: false, sharedTicker: false, resolution: 2, autoDensity: true })
    expect(log.tickerStarts).toBe(0)
    // Everything that shows sits in a positioned box after the canvas.
    expect(CSS).toMatch(/\.paperfx \{[^}]*position: absolute;[^}]*pointer-events: none;/)
    expect(CSS).toMatch(/\.page-foot \{\s*position: relative;/)
    expect(CSS).toMatch(/\.sheet \{\s*position: relative;/)
    // The shader paints the paper, the candle and the bleed.
    expect(log.shader?.gl.fragment).toMatch(/uniform sampler2D uPaper;[\s\S]*uLight[\s\S]*bleed\(/)
  })

  it('at rest it redraws with the flame, at most 10 frames a second, never 30', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'hi', true)] })
    run(h, 500)
    const before = log.renders
    expect(before).toBeGreaterThan(0)
    run(h, 10_000)
    const drawn = log.renders - before
    expect(drawn).toBeGreaterThanOrEqual(10)
    expect(drawn).toBeLessThanOrEqual(101)
  })

  it('stops while hidden and starts again when shown', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    run(h, 500)
    hide(true)
    const at = log.renders
    run(h, 10_000)
    expect(log.renders).toBe(at)
    hide(false)
    run(h, 500)
    expect(log.renders).toBeGreaterThan(at)
  })

  it('stops while its frame is out of sight (IntersectionObserver), as when hidden', async () => {
    const { PIXI, log } = fakePixi()
    let report: (e: Array<{ isIntersecting: boolean; intersectionRatio: number }>) => void = () => {}
    const IntersectionObserver = class {
      constructor(cb: typeof report) {
        report = cb
      }
      observe() {}
    }
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI, IntersectionObserver } })
    await vi.advanceTimersByTimeAsync(0)
    run(h, 500)
    report([{ isIntersecting: false, intersectionRatio: 0 }])
    expect(document.documentElement.classList.contains('asleep')).toBe(true)
    const at = log.renders
    run(h, 8000)
    expect(log.renders).toBe(at)
    report([{ isIntersecting: true, intersectionRatio: 1 }])
    run(h, 500)
    expect(log.renders).toBeGreaterThan(at)
  })

  it('reduced motion: one still frame (the candle at rest, no wet ink), then nothing', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    const h = await harness({ reduced: true, libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    expect(log.renders).toBe(1)
    expect(log.group?.uniforms.uLight[3]).toBeCloseTo(0.9, 5) // the flame at rest
    expect(log.group?.uniforms.uWetA[0]).toBe(0)
    run(h, 10_000)
    expect(log.renders).toBe(1)
  })

  it('a reply bleeding in wets the paper under its newest words, at most 30 frames a second; it dries and rests', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(
      () => ({ left: 120, top: 200, width: 60, height: 30, right: 180, bottom: 230, x: 120, y: 200, toJSON: () => ({}) }) as DOMRect,
    )
    const h = await harness({ libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(1, 'hi', true)] })
    run(h, 300)
    h.view('cortana::local', { items: [item(1, 'hi', true), item(2, 'A long reply that takes its time to write itself across the page.', false)] })
    const at = log.renders
    let wet = 0
    for (let t = 0; t < 1000; t += 8) {
      vi.advanceTimersByTime(8)
      h.advance(8)
      wet = Math.max(wet, log.group?.uniforms.uWetA[0] ?? 0)
    }
    expect(wet).toBeGreaterThan(0.5)
    const drawn = log.renders - at
    expect(drawn).toBeGreaterThan(15)
    expect(drawn).toBeLessThanOrEqual(31)
    // Done writing and dry: back to the flame's pace.
    run(h, 9000)
    expect(log.group?.uniforms.uWetA[0]).toBe(0)
    const rest = log.renders
    run(h, 3600)
    expect(log.renders - rest).toBeLessThanOrEqual(37) // at most 10 a second at rest
  })

  it('no WebGL: the canvas goes and the page is as it was', async () => {
    const { PIXI } = fakePixi({ fail: true })
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    expect(document.querySelector('canvas')).toBeNull()
    expect(document.documentElement.classList.contains('fx')).toBe(false)
    h.rows([row('cortana', 0)])
    expect(h.who()).toBe('Cortana')
  })

  // Rosson 2026-10-08: "The candle light background flicker is out of sync
  // with the page light flicker." One flame drives all three.
  it('one flame: the room’s candle, the page glow and the paper’s light peak together; the page follows, never leads', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    const room: number[] = []
    const glow: number[] = []
    const paper: number[] = []
    for (let f = 0; f < 600; f++) {
      vi.advanceTimersByTime(50)
      h.advance(50) // one flame frame each (20 a second)
      room.push(Number(h.$('candle').style.opacity))
      glow.push(Number(h.$('glow').style.opacity))
      paper.push(log.group?.uniforms.uLight[3] ?? NaN)
    }
    expect(new Set(room.map((v) => v.toFixed(2))).size).toBeGreaterThan(20) // it flickers
    // The paper's light is the page glow's light, drawn from the same sample
    // (or a frame behind it at rest, when the light barely moved).
    const page = glow.map((g) => (g - 0.45) / 0.55)
    for (let f = 1; f < page.length; f++) {
      const near = Math.min(Math.abs(paper[f] - page[f]), Math.abs(paper[f] - page[f - 1]))
      expect(near).toBeLessThan(0.02)
    }
    // The page follows the room: best match at lag 0 or 1 frame, never ahead.
    const corr = (lag: number) => {
      const a = room.slice(0, room.length - 2)
      const b = page.slice(lag, lag + a.length)
      const ma = a.reduce((x, y) => x + y, 0) / a.length
      const mb = b.reduce((x, y) => x + y, 0) / b.length
      let num = 0
      let da = 0
      let db = 0
      for (let i = 0; i < a.length; i++) {
        num += (a[i] - ma) * (b[i] - mb)
        da += (a[i] - ma) ** 2
        db += (b[i] - mb) ** 2
      }
      return num / Math.sqrt(da * db)
    }
    const lags = [0, 1, 2].map(corr)
    expect(Math.max(...lags)).toBeGreaterThan(0.9)
    expect(lags.indexOf(Math.max(...lags))).toBeLessThanOrEqual(1)
    // No CSS clock of their own: no keyframe flicker or gutter left.
    expect(CSS).not.toMatch(/animation: (flicker|gutter)/)
    expect(CSS).not.toMatch(/@keyframes (flicker|gutter)/)
    // Hidden: every candle holds the same rest, together.
    hide(true)
    expect(Number(h.$('candle').style.opacity)).toBeCloseTo(0.9, 5)
    expect(log.group?.uniforms.uLight[3]).toBeCloseTo(0.9, 5)
    expect(Number(h.$('glow').style.opacity)).toBeCloseTo(0.45 + 0.55 * 0.9, 3)
    const at = log.renders
    run(h, 3000)
    expect(log.renders).toBe(at)
    expect(Number(h.$('candle').style.opacity)).toBeCloseTo(0.9, 5)
    hide(false)
  })

  it('reduced motion: every candle still at the same rest', async () => {
    const { PIXI, log } = fakePixi()
    vi.useFakeTimers()
    const h = await harness({ reduced: true, libs: { PIXI } })
    await vi.advanceTimersByTimeAsync(0)
    run(h, 3000)
    expect(Number(h.$('candle').style.opacity)).toBeCloseTo(0.9, 5)
    expect(log.group?.uniforms.uLight[3]).toBeCloseTo(0.9, 5)
  })

  // Rosson 2026-10-08: "doesn't quite reach the bottom or the right side",
  // then "a tad too high on the bottom left". The paper canvas is the page's
  // first child, inset 0, sized from the page's own rect on every resize;
  // the page has no clip, mask or transform of its own (nothing for the
  // canvas to miss) and square edges; the page stack shows at the fore-edge
  // only, so nothing hangs below the page.
  it('the paper covers the whole page box after every resize; one square shape, nothing below the page', async () => {
    const { PIXI, log } = fakePixi()
    let observed: (() => void) | null = null
    const ResizeObserver = class {
      constructor(cb: () => void) {
        observed = cb
      }
      observe() {}
    }
    let size = { width: 720.4, height: 792.6 }
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return (this.id === 'page'
        ? { left: 140, top: 60, x: 140, y: 60, width: size.width, height: size.height, right: 140 + size.width, bottom: 60 + size.height, toJSON: () => ({}) }
        : { left: 0, top: 0, x: 0, y: 0, width: 0, height: 0, right: 0, bottom: 0, toJSON: () => ({}) }) as DOMRect
    })
    vi.useFakeTimers()
    const h = await harness({ libs: { PIXI, ResizeObserver } })
    await vi.advanceTimersByTimeAsync(0)
    expect(log.inits[0]).toMatchObject({ width: 721, height: 793 })
    for (const s of [{ width: 800.2, height: 880 }, { width: 512, height: 556.5 }]) {
      size = s
      ;(observed as unknown as () => void)()
      expect(log.resized.at(-1)).toEqual([Math.ceil(s.width), Math.ceil(s.height)])
      expect(log.group?.uniforms.uSize[0]).toBe(Math.ceil(s.width))
    }
    expect(h.$('page').firstElementChild?.className).toBe('paperfx')
    expect(CSS).toMatch(/\.paperfx \{[^}]*inset: 0;[^}]*width: 100% !important;[^}]*height: 100% !important;/)
    const pageRule = /\n\.page \{([^}]*)\}/.exec(CSS)?.[1] ?? ''
    expect(pageRule).toMatch(/border-radius: 0;/)
    expect(pageRule).not.toMatch(/clip-path|mask|transform/)
    // The stack: x offsets only (fore-edge), never a y offset below.
    const stack = [...pageRule.matchAll(/(\d+)px (\d+)(?:px)? 0 #/g)]
    expect(stack.length).toBe(3)
    for (const m of stack) expect(m[2]).toBe('0')
  })

})

describe('k2:diary@2: the room’s music (Tone.js)', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => false })
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  const QUIET = Math.pow(10, -24 / 20)

  it('nothing before K2 connects the frame; on arrival it fades in, quietly (-24 dB over 4 s)', async () => {
    const { Tone, log } = fakeTone()
    vi.useFakeTimers()
    const h = await harness({ libs: { Tone }, hello: 'late' })
    await vi.advanceTimersByTimeAsync(0)
    expect(log.contexts).toHaveLength(1)
    expect(log.contexts[0]).toMatchObject({ latencyHint: 'playback' })
    expect(log.resumes).toBe(1)
    expect(log.master.at(-1)?.[0]).toBeCloseTo(QUIET, 5)
    expect(log.master.at(-1)?.[1]).toBe(4)
    // Bells, creaks and wind follow on their own.
    await vi.advanceTimersByTimeAsync(60_000)
    expect(log.triggers).toBeGreaterThan(3)
    expect(h.$('hush').hidden).toBe(false)
  })

  it('held back until a gesture (an autoplay rule): silent on arrival, then the first click starts it', async () => {
    const { Tone, log } = fakeTone({ blocked: true })
    vi.useFakeTimers()
    const h = await harness({ libs: { Tone } })
    await vi.advanceTimersByTimeAsync(0)
    expect(log.resumes).toBe(1)
    expect(log.master).toEqual([])
    expect(log.nodes).toBe(0) // the room isn't even built yet
    await vi.advanceTimersByTimeAsync(30_000)
    expect(log.triggers).toBe(0)
    expect(h.$('hush').getAttribute('title')).toBe('Mute the music (M). Music: waiting for a click')
    // A key the computer still refuses: now the speaker says so.
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true }))
    await vi.advanceTimersByTimeAsync(0)
    expect(h.$('hush').getAttribute('title')).toBe('Mute the music (M). Music: blocked by the browser')
    log.allowed = true
    const before = log.resumes
    h.$('sheet').dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    // No await: the context resumed inside the event itself.
    expect(log.resumes).toBe(before + 1)
    await vi.advanceTimersByTimeAsync(0)
    expect(log.master.at(-1)?.[0]).toBeCloseTo(QUIET, 5)
    expect(h.$('hush').getAttribute('title')).toBe('Mute the music (M). Music: playing')
    // Once it runs, more clicks change nothing.
    const resumed = log.resumes
    h.$('sheet').dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }))
    await vi.advanceTimersByTimeAsync(0)
    expect(log.resumes).toBe(resumed)
  })

  it('hidden: it fades out and the audio rests; shown again: it comes back', async () => {
    const { Tone, log } = fakeTone()
    vi.useFakeTimers()
    await harness({ libs: { Tone } })
    await vi.advanceTimersByTimeAsync(0)
    hide(true)
    expect(log.master.at(-1)).toEqual([0, 0.8])
    await vi.advanceTimersByTimeAsync(1000)
    expect(log.suspends).toBe(1)
    const triggers = log.triggers
    await vi.advanceTimersByTimeAsync(60_000)
    expect(log.triggers).toBe(triggers) // no bells in an empty room
    hide(false)
    await vi.advanceTimersByTimeAsync(0)
    expect(log.resumes).toBe(2)
    expect(log.master.at(-1)?.[0]).toBeCloseTo(QUIET, 5)
  })

  it('leaving the Garden (K2 removes the frame): it stops at once', async () => {
    const { Tone, log } = fakeTone()
    vi.useFakeTimers()
    const h = await harness({ libs: { Tone } })
    await vi.advanceTimersByTimeAsync(0)
    h.windowEvent('pagehide')
    expect(log.closes).toBe(1)
    const triggers = log.triggers
    await vi.advanceTimersByTimeAsync(60_000)
    expect(log.triggers).toBe(triggers)
  })

  it('the speaker at the top left mutes and unmutes; M does too, but never while you write', async () => {
    const { Tone, log } = fakeTone()
    vi.useFakeTimers()
    const h = await harness({ libs: { Tone, PerfectFreehand: PF } })
    await vi.advanceTimersByTimeAsync(0)
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    const hush = h.$('hush')
    expect(hush.parentElement).toBe(h.$('desk')) // over the room, not on the page
    expect(hush.getAttribute('aria-label')).toBe('Mute the music (M). Music: playing')
    expect(hush.getAttribute('title')).toBe(hush.getAttribute('aria-label'))
    expect(hush.getAttribute('aria-pressed')).toBe('false')
    expect(hush.querySelectorAll('svg path.cone, svg path.wave, svg path.cross').length).toBe(4)
    hush.click()
    expect(hush.getAttribute('aria-pressed')).toBe('true')
    expect(hush.getAttribute('aria-label')).toBe('Play the music (M). Music: muted')
    expect(log.master.at(-1)).toEqual([0, 0.8])
    await vi.advanceTimersByTimeAsync(1000)
    expect(log.suspends).toBe(1)
    // M, with the pen not in hand.
    h.key('m')
    await vi.advanceTimersByTimeAsync(0)
    expect(hush.getAttribute('aria-pressed')).toBe('false')
    expect(log.master.at(-1)?.[0]).toBeCloseTo(QUIET, 5)
    // M in the pen is a letter.
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'm', bubbles: true }))
    expect(hush.getAttribute('aria-pressed')).toBe('false')
    // Hidden while muted, then shown: still muted, still silent.
    hush.click()
    hide(true)
    hide(false)
    await vi.advanceTimersByTimeAsync(0)
    expect(hush.getAttribute('aria-pressed')).toBe('true')
    expect(log.master.at(-1)?.[0]).toBe(0)
    // Top left, just under K2's floating 52 px top band (which takes every
    // click above it) and the Garden switcher in it; dim until reached for.
    expect(CSS).toMatch(/\.hush \{[^}]*position: absolute;[^}]*top: 58px;[^}]*left: 14px;[^}]*z-index: 6;/)
    // Below K2's 52 px drag band, which would take its clicks.
    expect(Number(/\n\.hush \{[^}]*top: (\d+)px;/.exec(CSS)?.[1])).toBeGreaterThanOrEqual(52)
    // The same glass tile as K2's menu and switcher (zen-glass.ts, ZenMenu).
    const glass = /\n\.hush \{([^}]*)\}/.exec(CSS)?.[1] ?? ''
    for (const d of ['height: 36px;', 'width: 36px;', 'border-radius: 999px;', 'background: var(--zen-surface', 'backdrop-filter: none;', 'inset 0 1px 0 color-mix(in srgb, white 22%, transparent), 0 10px 30px color-mix(in srgb, black 8%, transparent)', 'color: var(--zen-text']) {
      expect(glass).toContain(d)
    }
    expect(CSS).toMatch(/\.hush:hover, \.hush:active \{ background: var\(--zen-surface-raised/)
    expect(/\.hush \{[^}]*\bright:/.test(CSS)).toBe(false)
  })

  it('a Garden can start it muted (config music = "off"); the mute is kept while the frame lives', async () => {
    const { Tone, log } = fakeTone()
    vi.useFakeTimers()
    const h = await harness({ libs: { Tone }, config: { music: 'off' } })
    await vi.advanceTimersByTimeAsync(0)
    expect(log.resumes).toBe(0)
    expect(h.$('hush').getAttribute('aria-pressed')).toBe('true')
  })

  it('no Tone.js, or no Web Audio: silent, and the speaker says which', async () => {
    const h1 = await harness()
    expect(h1.$('hush').hidden).toBe(false)
    expect(h1.$('hush').getAttribute('title')).toMatch(/Music: the music library didn’t load$/)
    document.body.innerHTML = ''
    const { Tone } = fakeTone()
    const Broken = { ...Tone, Context: class { constructor() { throw new Error('no audio') } } }
    vi.useFakeTimers()
    const h2 = await harness({ libs: { Tone: Broken } })
    await vi.advanceTimersByTimeAsync(0)
    expect(h2.$('hush').hidden).toBe(false)
    expect(h2.$('hush').classList.contains('off')).toBe(true)
    expect(h2.$('hush').getAttribute('title')).toMatch(/Music: no Web Audio here$/)
  })

  it('only Tone’s plain nodes: nothing that needs an AudioWorklet (refused in a sealed widget)', () => {
    for (const bad of ['Freeverb', 'JCReverb', 'FeedbackCombFilter', 'LowpassCombFilter', 'BitCrusher', 'Tone.start', 'ToneAudioWorklet']) {
      expect(JS).not.toContain(bad)
    }
  })
})

describe('k2:diary@2: the low drone breathes', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  // Rosson 2026-10-08: "The underlying low tone goes for a bit too long".
  it('swells in, holds 8-15 s, fades, then rests 10-25 s before the next swell; never one endless tone', async () => {
    document.body.innerHTML = ''
    const { Tone, log } = fakeTone()
    vi.useFakeTimers()
    await harness({ libs: { Tone } })
    await vi.advanceTimersByTimeAsync(0)
    await vi.advanceTimersByTimeAsync(10 * 60_000)
    // The drone's gain: the one that swells to 0.75..1 and back to 0.
    const drone = log.gains.find((r) => r.some(([v]) => v >= 0.7 && v <= 1)) ?? []
    const ups = drone.filter(([v]) => v > 0)
    const downs = drone.filter(([v], i) => v === 0 && i > 0)
    expect(ups.length).toBeGreaterThanOrEqual(12) // a breath every 25-52 s
    expect(ups.length).toBeLessThanOrEqual(26)
    for (const [v, secs] of ups) {
      expect(v).toBeGreaterThanOrEqual(0.75)
      expect(v).toBeLessThanOrEqual(1)
      expect(secs).toBeGreaterThanOrEqual(3)
      expect(secs).toBeLessThanOrEqual(5)
    }
    for (let i = 0; i < ups.length - 1; i++) {
      const fade = downs.find(([, , at]) => at > ups[i][2]) ?? [0, 0, Infinity]
      const held = (fade[2] - ups[i][2]) / 1000 // the rise and the hold
      expect(held).toBeGreaterThanOrEqual(3 + 8 - 0.01)
      expect(held).toBeLessThanOrEqual(5 + 15 + 0.01)
      const rest = (ups[i + 1][2] - fade[2]) / 1000 - fade[1] // after the fade
      expect(rest).toBeGreaterThanOrEqual(10 - 0.01)
      expect(rest).toBeLessThanOrEqual(25 + 0.01)
    }
    expect(JS).toContain('var DRONE_ROOTS = [')
  }, 30_000)
})

describe('k2:diary@2: square pages whose corners curl, never dog-eared', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })

  it('each turn control holds an SVG curl drawn in code: what is beneath, a cast shadow, the underside, its crease', async () => {
    const h = await harness({ reduced: true })
    h.rows([row('cortana', 0), row('nora', 1)])
    for (const id of ['prev', 'next']) {
      const svg = h.$(id).querySelector('svg.curl')
      expect(svg?.namespaceURI).toBe('http://www.w3.org/2000/svg')
      expect(svg?.getAttribute('aria-hidden')).toBe('true')
      expect([...(svg?.querySelectorAll('.lift > path') ?? [])].map((p) => p.getAttribute('class'))).toEqual(['beneath', 'cast', 'under', 'crease'])
      // The crease is a curve, not a straight diagonal.
      expect(svg?.querySelector('.crease')?.getAttribute('d')).toMatch(/^M[\d. ]+ Q[\d. ]+$/)
      // The underside: lighter at the crease, darker toward the tip.
      const stops = [...(svg?.querySelectorAll('[id^="curl-under"] stop') ?? [])].map((s) => s.getAttribute('stop-color'))
      expect(stops).toEqual(['#f7ecd2', '#e2cc9e', '#bfa271'])
    }
    h.key('ArrowRight')
    expect(h.who()).toBe('Nora')
  })

  it('plain square paper at rest; hover or focus lifts the curl in 200 ms; touch keeps a faint hint; reduced motion a still curl', () => {
    expect(CSS).not.toMatch(/\.corner[^{]*::before/)
    expect(CSS).not.toMatch(/var\(--night\) 0 50%/)
    expect(CSS).toMatch(/\.page \{[^}]*border-radius: 0;/)
    expect(CSS).toMatch(/\.leaf \.back \{[^}]*border-radius: 0;/)
    expect(CSS).toMatch(/\.curl \.lift \{[^}]*transform-origin: 64px 64px;[^}]*transform: scale\(0\);[^}]*transition: transform 200ms/)
    expect(CSS).toMatch(/\.corner:hover \.curl \.lift, \.corner:focus-visible \.curl \.lift \{ transform: scale\(1\); \}/)
    expect(CSS).toMatch(/@media \(hover: none\) \{\s*\.curl \.lift \{ transform: scale\(0\.34\);/)
    expect(CSS).toMatch(/:root\.reduced \.curl \.lift,[^{]*\{ transition: none; transform: scale\(0\.4\); \}/)
    expect(CSS).toMatch(/\.corner:disabled \.curl \{ display: none; \}/)
  })
})

describe('k2:diary@2: the Diary page template', () => {
  // Rosson 2026-10-08: "We now have two diary lab buttons". One ⋯ menu,
  // moved to the top left, holds the one Garden switcher and the toggle.
  it('has exactly one Garden switcher, inside the one menu at the top left', () => {
    const toml = readFileSync(resolve(__dirname, '../../../../crates/k2-core/src/zen/garden-catalog/diary-2.toml'), 'utf8')
    const widgets = toml.split('[[widget]]').slice(1).map((w) => w.split(/\n\[\[/)[0])
    const switchers = widgets.filter((w) => /kind = "garden-switcher"/.test(w))
    expect(switchers).toHaveLength(1)
    expect(switchers[0]).toMatch(/slot = "menu"\nmenu = "more"/)
    const menus = widgets.filter((w) => /kind = "menu"/.test(w))
    expect(menus).toHaveLength(1)
    expect(menus[0]).toMatch(/id = "more"\nkind = "menu"\nslot = "top"\nalign = "start"/)
    expect(toml.match(/\[\[control\]\]\nkind = "garden-switcher"\nplacement = "menu:more"/g)).toHaveLength(1)
  })
})
