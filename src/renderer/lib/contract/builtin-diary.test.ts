// @vitest-environment jsdom
// The built-in Diary widget, k2:diary@1 (prd-zen-user-widgets-v2 UWB21,
// UWB24; the haunted Diary, Rosson 2026-10-08), run byte for byte against a
// fake `k2` object with a hand-cranked frame clock. The real frame, bridge
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
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const DIR = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets/diary')
const HTML = readFileSync(resolve(DIR, 'index.html'), 'utf8')
const JS = readFileSync(resolve(DIR, 'diary.js'), 'utf8')

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

function harness(opts: { reduced?: boolean; can?: (v: string) => boolean } = {}) {
  let rowsCb: Cb | null = null
  const threads = new Map<string, Cb>()
  let frames: Array<(t: number) => void> = []
  let now = 0
  const k2 = {
    config: {},
    motion: { reduced: opts.reduced === true },
    widget: { id: 'diary', name: 'Diary', garden: 'g-test0001' },
    can: vi.fn((v: string) => (opts.can ? opts.can(v) : true)),
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

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('k2:diary@1, the haunted Diary', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('one page per agent on this computer: a remote agent never gets a page; labels stay text; ready once', () => {
    const h = harness({ reduced: true })
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
    const h = harness({ reduced: true })
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

  it('arrows don’t turn the page while you are writing; PageDown does', () => {
    const h = harness({ reduced: true })
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

  it('with motion, a turn lifts the old page as a leaf over the new one, then follows the new Thread', () => {
    const h = harness()
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

  it('grab the corner and drag: a tiny tug lifts nothing, a real drag turns the page', () => {
    const h = harness()
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

  it('history is shown whole; a new reply bleeds in as handwriting; a tap shows it all', () => {
    const h = harness()
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

  it('a page with no history still bleeds in its first reply', () => {
    const h = harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    expect(h.$('note').textContent).toBe('A blank page. Write something, and see what writes back.')
    h.view('cortana::local', { items: [item(7, 'I have been waiting.', false)] })
    expect(document.querySelector('.entry.bleeding')).not.toBeNull()
  })

  it('reduced motion: the turn is instant with no leaf, and replies arrive whole', () => {
    const h = harness({ reduced: true })
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

  it('sending soaks the ink into the page and fades your words', async () => {
    const h = harness()
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [] })
    const ink = h.$('ink') as HTMLTextAreaElement
    expect(h.$('pen').hidden).toBe(false)
    ink.value = 'Dear Cortana, how goes it?'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.post).toHaveBeenCalledWith('cortana::local', 'Dear Cortana, how goes it?')
    expect(h.$('soak').classList.contains('on')).toBe(true)
    await flush()
    h.view('cortana::local', { items: [{ ...item(99, 'Dear Cortana, how goes it?', true), id: 'm99' }] })
    expect(document.querySelector('.entry.mine.absorbed')).not.toBeNull()
  })

  it('a refused post gives the words back and says why, in the Diary’s words', async () => {
    const h = harness()
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

  it('working stirs the ink; needs-you lifts the bookmark, and the corner toward a calling page smoulders', () => {
    const h = harness({ reduced: true })
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
    const h = harness({ reduced: true })
    h.rows([row('cortana', 0)])
    h.view('cortana::local', { items: [item(5, 'first', true), item(6, 'r1', false)], hasMore: true })
    h.$('sheet').dispatchEvent(new Event('scroll'))
    expect(h.k2.thread.read).toHaveBeenCalledWith('cortana::local', { beforeSeq: 5, limit: 50 })
    await flush()
    await flush()
    expect(h.$('entries').textContent).toContain('the very first words')
  })

  it('choice cards answer through thread.answer; secret cards stay in K2', () => {
    const h = harness()
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

  it('no agents on this computer: a blank page that says so', () => {
    const h = harness()
    h.rows([remote('julie', 0)])
    expect(h.$('caption').textContent).toBe('the pages are blank')
    expect(h.$('note').textContent).toBe('No spirits dwell on this computer yet. Add an agent in K2, and a page will appear for it.')
    expect(h.k2.thread.subscribe).not.toHaveBeenCalled()
    expect(h.$('pen').hidden).toBe(true)
  })

  it('without agents:read it asks to be allowed, and is still ready', () => {
    const h = harness({ can: (v) => v !== 'agents.subscribe' })
    expect(h.k2.agents.subscribe).not.toHaveBeenCalled()
    expect(h.$('note').textContent).toBe('Allow this Diary in K2 to see the agents on this computer.')
    expect(h.k2.ready).toHaveBeenCalledTimes(1)
  })
})
