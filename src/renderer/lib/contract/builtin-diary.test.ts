// @vitest-environment jsdom
// The built-in Diary widget, k2:diary@1 (prd-zen-user-widgets-v2 UWB21,
// UWB24), run against a fake `k2` object: the contents page, search, the
// handwriting reveal, sending, needs-you, page turns and history. The real
// frame, bridge and caps are B4's (TUW3.x); this pins the widget's own
// behaviour. Agent text must stay text: a label carrying markup never
// becomes an element.
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

const row = (n: number, extra: Partial<Row> = {}): Row => ({
  address: `agent${n}::example.test`,
  label: `Agent ${n}`,
  index: n,
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

const item = (seq: number, body: string, mine: boolean, extra: Record<string, unknown> = {}) => ({
  seq,
  id: `m${seq}`,
  doc: { id: `m${seq}`, kind: 'message', from: mine ? 'alice' : 'agent1', body, via: mine ? 'compose' : 'thread', created_at: 1_800_000_000 + seq, ...extra },
})

type Cb = (v: unknown) => void

function harness(opts: { reduced?: boolean; can?: (v: string) => boolean } = {}) {
  let rowsCb: Cb | null = null
  let threadCb: Cb | null = null
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
      subscribe: vi.fn((_a: string, cb: Cb) => {
        threadCb = cb
        return vi.fn()
      }),
      read: vi.fn(async () => ({ items: [item(1, 'the very first words', false)], hasMore: false })),
      post: vi.fn(async () => ({ id: 'm99', seq: 99 })),
      answer: vi.fn(async () => null),
    },
    conversation: { open: vi.fn(async () => null) },
    theme: {
      get: vi.fn(async () => ({ theme: { scheme: 'dark', vars: { '--zen-accent': '#123456' } }, chrome: { corners: null, stoplights: null }, motion: { reduced: false } })),
      changed: vi.fn(() => () => {}),
    },
  }
  document.documentElement.removeAttribute('data-zen-scheme')
  document.documentElement.className = ''
  document.body.innerHTML = HTML.slice(HTML.indexOf('<main'), HTML.indexOf('<script'))
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
  return {
    k2,
    rows: (r: Row[]) => (rowsCb as Cb)(r),
    view: (v: Record<string, unknown>) => (threadCb as Cb)({ address: 'agent1::example.test', phase: 'ready', note: null, hasMore: false, ...v }),
    advance(ms: number) {
      now += ms
      const run = frames
      frames = []
      run.forEach((f) => f(now))
    },
    $: (id: string) => document.getElementById(id) as HTMLElement,
  }
}

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('k2:diary@1', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('lists agents on the contents page as text, and says it is ready once', () => {
    const h = harness()
    h.rows([row(1, { label: '<img src=x onerror=alert(1)>' }), row(2, { needsYou: true, activity: 'needs-you' })])
    const lis = h.$('toc').querySelectorAll('li')
    expect(lis).toHaveLength(2)
    expect(lis[0].querySelector('.name')?.textContent).toBe('<img src=x onerror=alert(1)>')
    expect(document.querySelector('#toc img')).toBeNull()
    expect(lis[1].classList.contains('needs')).toBe(true)
    h.rows([row(1), row(2)])
    expect(h.k2.ready).toHaveBeenCalledTimes(1)
    expect(h.$('leaf').hidden).toBe(true)
  })

  it('search filters the contents, and Enter opens the first match', () => {
    const h = harness()
    h.rows([row(1), row(2, { label: 'Scout', server: 'box.example.test' })])
    const search = h.$('search') as HTMLInputElement
    search.value = 'box'
    search.dispatchEvent(new Event('input'))
    expect(h.$('toc').querySelectorAll('li')).toHaveLength(1)
    search.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.subscribe).toHaveBeenCalledWith('agent2::example.test', expect.any(Function))
    expect(h.k2.conversation.open).toHaveBeenCalledWith('agent2::example.test')
  })

  it('opens the only agent straight away; history is shown whole, a new reply is handwritten', () => {
    const h = harness()
    h.rows([row(1)])
    expect(h.k2.thread.subscribe).toHaveBeenCalledWith('agent1::example.test', expect.any(Function))
    h.view({ items: [item(1, 'hello', true), item(2, 'old reply', false)] })
    expect(h.$('entries').textContent).toContain('old reply')
    expect(document.querySelector('.entry.writing')).toBeNull()
    const reply = 'A new reply, written out slowly in ink.'
    h.view({ items: [item(1, 'hello', true), item(2, 'old reply', false), item(3, 'next', true), item(4, reply, false)] })
    const writing = document.querySelector('.entry.writing .body') as HTMLElement
    expect(writing).not.toBeNull()
    h.advance(200)
    expect((writing.textContent ?? '').length).toBeGreaterThan(0)
    expect((writing.textContent ?? '').length).toBeLessThan(reply.length)
    // A later push (the turn ends) keeps the pen going, not restarting it.
    h.view({ items: [item(1, 'hello', true), item(2, 'old reply', false), item(3, 'next', true), item(4, reply, false)], turn: null })
    expect(document.querySelector('.entry.writing')).not.toBeNull()
    // A tap shows it all.
    h.$('sheet').dispatchEvent(new MouseEvent('click', { bubbles: true }))
    expect(document.querySelector('.entry.writing')).toBeNull()
    expect(h.$('entries').textContent).toContain(reply)
  })

  it('reduced motion writes replies at once', () => {
    const h = harness({ reduced: true })
    h.rows([row(1)])
    h.view({ items: [item(1, 'hi', true)] })
    h.view({ items: [item(1, 'hi', true), item(2, 'whole at once', false)] })
    expect(document.querySelector('.entry.writing')).toBeNull()
    expect(h.$('entries').textContent).toContain('whole at once')
  })

  it('working shows the ink shimmer; needs-you lifts the bookmark', () => {
    const h = harness()
    h.rows([row(1)])
    h.view({ items: [item(1, 'hi', true)], turn: { state: 'working', since: 1 } })
    expect(h.$('shimmer').hidden).toBe(false)
    expect(h.$('bookmark').hidden).toBe(true)
    h.view({ items: [item(1, 'hi', true)], turn: { state: 'needs-you', since: 1 } })
    expect(h.$('shimmer').hidden).toBe(true)
    expect(h.$('bookmark').hidden).toBe(false)
  })

  it('sending posts the text, soaks the ink into the page, and fades your words', async () => {
    const h = harness()
    h.rows([row(1)])
    h.view({ items: [] })
    const ink = h.$('ink') as HTMLTextAreaElement
    expect(h.$('pen').hidden).toBe(false)
    ink.value = 'Dear agent, how goes it?'
    ink.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }))
    expect(h.k2.thread.post).toHaveBeenCalledWith('agent1::example.test', 'Dear agent, how goes it?')
    expect(ink.value).toBe('')
    expect(h.$('soak').classList.contains('on')).toBe(true)
    await flush()
    h.view({ items: [{ ...item(99, 'Dear agent, how goes it?', true), id: 'm99' }] })
    expect(document.querySelector('.entry.mine.absorbed')).not.toBeNull()
  })

  it('a refused post gives the words back and says why', async () => {
    const h = harness()
    h.rows([row(1)])
    h.view({ items: [] })
    h.k2.thread.post.mockRejectedValueOnce(Object.assign(new Error('off'), { code: 'sending_off' }))
    const ink = h.$('ink') as HTMLTextAreaElement
    ink.value = 'kept'
    h.$('pen').dispatchEvent(new Event('submit', { cancelable: true }))
    await flush()
    await flush()
    expect(ink.value).toBe('kept')
    expect(h.$('note').textContent).toBe('Sending is turned off for this Diary.')
  })

  it('pages turn back through history, then read older pages with beforeSeq', async () => {
    const h = harness()
    h.rows([row(1)])
    h.view({ items: [item(5, 'first', true), item(6, 'r1', false), item(7, 'second', true), item(8, 'r2', false)], hasMore: true })
    expect(h.$('folio').textContent).toBe('page 2 of 2')
    expect((h.$('next') as HTMLButtonElement).disabled).toBe(true)
    h.$('prev').click()
    expect(h.$('folio').textContent).toBe('page 1 of 2')
    expect(h.$('pen').hidden).toBe(true)
    h.$('prev').click()
    expect(h.k2.thread.read).toHaveBeenCalledWith('agent1::example.test', { beforeSeq: 5, limit: 50 })
    await flush()
    await flush()
    expect(h.$('folio').textContent).toBe('page 1 of 3')
    expect(h.$('entries').textContent).toContain('the very first words')
    h.$('next').click()
    h.$('next').click()
    expect(h.$('folio').textContent).toBe('page 3 of 3')
    expect(h.$('pen').hidden).toBe(false)
  })

  it('choice cards answer through thread.answer; secret cards stay in K2', async () => {
    const h = harness()
    h.rows([row(1)])
    const choice = { prompt: 'Ship it?', options: [{ label: 'Yes' }, { label: 'No' }], allow_custom: false, status: 'pending' }
    h.view({
      items: [item(1, 'q', true), item(2, '', false, { kind: 'ask', choice }), item(3, '', false, { kind: 'secret' })],
    })
    const yes = [...document.querySelectorAll('.choice button')].find((b) => b.textContent === 'Yes') as HTMLButtonElement
    yes.click()
    expect(h.k2.thread.answer).toHaveBeenCalledWith('agent1::example.test', 'm2', 'Yes')
    expect(h.$('entries').textContent).toContain('Answer it in K2’s Thread')
  })

  it('takes the Garden theme', async () => {
    harness()
    await flush()
    expect(document.documentElement.getAttribute('data-zen-scheme')).toBe('dark')
    expect(document.documentElement.style.getPropertyValue('--zen-accent')).toBe('#123456')
  })

  it('without agents:read it asks to be allowed, and is still ready', () => {
    const h = harness({ can: (v) => v !== 'agents.subscribe' })
    expect(h.k2.agents.subscribe).not.toHaveBeenCalled()
    expect(h.$('toc-empty').textContent).toBe('Allow this Diary to see your agents to begin.')
    expect(h.k2.ready).toHaveBeenCalledTimes(1)
  })
})
