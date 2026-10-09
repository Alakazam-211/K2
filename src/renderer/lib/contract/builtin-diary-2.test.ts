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

const flush = () => new Promise((r) => setTimeout(r, 0))

async function harness(opts: { reduced?: boolean; can?: (v: string) => boolean; hello?: 'late' | 'never' } = {}) {
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
    config: {},
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
  const win = { k2 } as unknown as Window
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
    expect(ghost.hidden).toBe(true)
    tick(3600)
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

  it('stops the instant you focus or type, and comes back once the pen is empty and left alone', async () => {
    const { ink, ghost, tick } = await haunted()
    tick(3600)
    tick(2000)
    expect(ghost.hidden).toBe(false)
    ink.focus()
    expect(ghost.hidden).toBe(true)
    expect(ghost.textContent).toBe('')
    expect(ink.placeholder).toBe('Write here…')
    tick(30_000)
    expect(ghost.hidden).toBe(true)
    ink.value = 'I am'
    ink.dispatchEvent(new Event('input'))
    ink.value = ''
    ink.dispatchEvent(new Event('input'))
    tick(30_000)
    expect(ghost.hidden).toBe(true)
    ink.blur()
    tick(2000)
    expect(ghost.hidden).toBe(true)
    tick(1600)
    expect(ghost.hidden).toBe(false)
    // Typing (without a focus event) stops it too.
    tick(2000)
    ink.value = 'x'
    ink.dispatchEvent(new Event('input'))
    expect(ghost.hidden).toBe(true)
    tick(30_000)
    expect(ghost.hidden).toBe(true)
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
