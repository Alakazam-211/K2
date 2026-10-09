// @vitest-environment jsdom
// Widgets on K2's REAL frame runtime (sdk/generated/k2-frame.js) in the REAL
// order (the 0.45.1 smoke bug, z3mbpZ): K2 inlines the runtime first, the
// widget's own script runs next, and only then, on the frame's `load`
// (ZenCustomFrame onLoad), does the host post the hello with the port. A
// widget that called `k2.can()` at startup saw false and drew "the diary is
// sealed" on every real Home, while tests that posted the hello first passed.
//
// `realFrame()` posts the hello LATE by default (after the widget script and
// a turn of the event loop); `hello: 'never'` leaves it out. There is no
// hello-first mode on purpose: that order never happens in K2.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { createContext, runInContext } from 'node:vm'
import { MessageChannel, type MessagePort } from 'node:worker_threads'
import { afterEach, describe, expect, it } from 'vitest'
import { k2FrameScript } from '../zen/zen-custom-prelude'

const ZEN = resolve(__dirname, '../../../../crates/k2-core/src/zen')
const read = (p: string): string => readFileSync(resolve(ZEN, p), 'utf8')

type Listener = (e: unknown) => void
type Msg = Record<string, any>

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

const row = (handle: string, index: number): Row => ({
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
})

const ports: MessagePort[] = []
afterEach(() => {
  for (const p of ports.splice(0)) p.close()
  document.body.innerHTML = ''
  document.documentElement.className = ''
})

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 5))

async function until(what: string, ok: () => boolean): Promise<void> {
  for (let i = 0; i < 200; i++) {
    if (ok()) return
    await tick()
  }
  throw new Error(`timed out waiting for: ${what}`)
}

interface Frame {
  /** What the widget sent the host, in order (hello port only). */
  heard: Msg[]
  /** Push a value to the widget's live subscription of `verb`. */
  push(verb: string, value: unknown, arg?: unknown): void
  subscribed(verb: string): Msg[]
  ready(): number
  /** Expire the runtime's connect timer (no hello in 10 s). */
  expire(): void
  $(id: string): HTMLElement
}

/**
 * One sealed frame: the real runtime, then the widget's script, then (late,
 * on "load") the hello. Answers `thread.post` and `conversation.open` ok.
 */
async function realFrame(opts: {
  html: string
  run: (k2: unknown) => void
  caps: string[]
  hello?: 'late' | 'never'
}): Promise<Frame> {
  document.body.innerHTML = opts.html.slice(opts.html.indexOf('<body>') + 6, opts.html.indexOf('<script'))
  const listeners = new Map<string, Listener[]>()
  const timers: Array<() => void> = []
  const parent = {}
  const g: Record<string, unknown> = {
    addEventListener: (t: string, f: Listener) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
    removeEventListener: (t: string, f: Listener) => listeners.set(t, (listeners.get(t) ?? []).filter((x) => x !== f)),
    setTimeout: (f: () => void) => {
      timers.push(f)
      return timers.length
    },
    parent,
    navigator: { platform: 'MacIntel' },
    document,
    console,
  }
  g.self = g
  runInContext(k2FrameScript(), createContext(g))
  const k2 = g.k2
  if (!k2) throw new Error('the runtime set no k2')

  // The widget's own script: it runs before K2 connects the frame.
  opts.run(k2)

  const heard: Msg[] = []
  const ch = new MessageChannel()
  ports.push(ch.port1, ch.port2)
  ch.port1.on('message', (m: Msg) => {
    heard.push(m)
    if (typeof m.id === 'number' && m.verb === 'thread.post') ch.port1.postMessage({ id: m.id, ok: true, value: { id: 'm99', seq: 99 } })
    else if (typeof m.id === 'number') ch.port1.postMessage({ id: m.id, ok: true, value: null })
  })
  // A turn of the event loop: the frame "loads", then the host says hello.
  await tick()
  if (opts.hello !== 'never') {
    for (const f of listeners.get('message') ?? []) {
      f({
        source: parent,
        data: {
          k2: 'hello',
          v: 1,
          caps: opts.caps,
          features: ['zen-v1', 'zen-gardens-v1', 'zen-widgets-v1'],
          widget: { id: 'w', name: 'W', garden: 'g-test0001' },
          config: {},
          motion: { reduced: true },
        },
        ports: [ch.port2],
      })
    }
  }
  await tick()
  const subscribed = (verb: string) => heard.filter((m) => typeof m.sub === 'number' && m.verb === verb)
  return {
    heard,
    subscribed,
    push(verb, value, arg) {
      const subs = subscribed(verb).filter((m) => arg === undefined || m.args[0] === arg)
      const last = subs[subs.length - 1]
      if (!last) throw new Error(`the widget never subscribed to ${verb}${arg === undefined ? '' : ` ${String(arg)}`}`)
      ch.port1.postMessage({ sub: last.sub, value })
    },
    ready: () => heard.filter((m) => m.ready === true).length,
    expire: () => {
      for (const t of timers.splice(0)) t()
    },
    $: (id: string) => {
      const el = document.getElementById(id)
      if (!el) throw new Error(`no #${id}`)
      return el
    },
  }
}

const noFrame = { raf: (_f: unknown) => 0, perf: { now: () => 0 } }

const diaryRunner = (dir: string) => (k2: unknown): void => {
  new Function('window', 'document', 'requestAnimationFrame', 'cancelAnimationFrame', 'performance', read(`builtin_widgets/${dir}/diary.js`))(
    { k2 },
    document,
    noFrame.raf,
    () => {},
    noFrame.perf,
  )
}

const DIARY_CAPS = ['agents:read', 'thread:read', 'thread:post']

// Every released Diary version (`k2:diary@1` stays byte for byte; @2 adds
// dark ink, markdown and the ghost in the pen).
describe.each([
  ['k2:diary@1', 'diary'],
  ['k2:diary@2', 'diary-2'],
])('%s on the real runtime, hello after the script', (_id, dir) => {
  const runDiary = diaryRunner(dir)
  it('renders a page per local agent and posts to it (never "sealed")', async () => {
    const f = await realFrame({ html: read(`builtin_widgets/${dir}/index.html`), run: runDiary, caps: DIARY_CAPS })
    await until('the Diary subscribes to agents', () => f.subscribed('agents.subscribe').length === 1)
    expect(f.$('caption').textContent).not.toBe('the diary is sealed')
    f.push('agents.subscribe', [row('cortana', 0), row('nora', 1)])
    await until('a page for Cortana', () => f.$('who').textContent === 'Cortana')
    expect(f.$('caption').textContent).toBe('you are writing to')
    expect(f.$('folio').textContent).toBe('page i of ii')
    await until('the Thread subscription', () => f.subscribed('thread.subscribe').length === 1)
    expect(f.subscribed('thread.subscribe')[0].args).toEqual(['cortana::local'])
    f.push('thread.subscribe', { address: 'cortana::local', phase: 'ready', note: null, hasMore: false, turn: null, items: [] }, 'cortana::local')
    await until('the pen shows', () => !(f.$('pen') as HTMLFormElement).hidden)
    const ink = f.$('ink') as HTMLTextAreaElement
    ink.value = 'Are you there?'
    f.$('pen').dispatchEvent(new Event('submit', { cancelable: true }))
    await until('the post reaches K2', () => f.heard.some((m) => m.verb === 'thread.post'))
    expect(f.heard.filter((m) => m.verb === 'thread.post').map((m) => m.args)).toEqual([['cortana::local', 'Are you there?']])
    expect(f.ready()).toBe(1)
    expect(document.body.textContent).not.toMatch(/Allow|permission/i)
  })

  it('no hello in 10 s: the page says it can’t reach your agents, and asks for nothing', async () => {
    const f = await realFrame({ html: read(`builtin_widgets/${dir}/index.html`), run: runDiary, caps: DIARY_CAPS, hello: 'never' })
    expect(f.$('caption').textContent).toBe('the diary is waking')
    f.expire()
    await until('the sealed note', () => f.$('caption').textContent === 'the diary is sealed')
    expect(f.$('note').textContent).toBe('The diary can’t reach your agents right now.')
    expect(f.heard).toEqual([])
    expect(document.body.textContent).not.toMatch(/Allow|permission/i)
  })
})

describe('examples on the real runtime, hello after the script', () => {
  it('arcade: draws a hero per agent and talks to it', async () => {
    const f = await realFrame({
      html: read('examples/arcade/index.html'),
      run: (k2) => new Function('k2', 'document', read('examples/arcade/arcade.js'))(k2, document),
      caps: ['agents:read', 'thread:read', 'thread:post'],
    })
    await until('the arcade subscribes to agents', () => f.subscribed('agents.subscribe').length === 1)
    f.push('agents.subscribe', [row('cortana', 0)])
    await until('a hero', () => document.querySelectorAll('.hero').length === 1)
    expect((f.$('empty') as HTMLElement).hidden).toBe(true)
    ;(document.querySelector('.hero') as HTMLButtonElement).click()
    expect((f.$('talk') as HTMLFormElement).hidden).toBe(false)
    ;(f.$('talk-text') as HTMLInputElement).value = 'hi'
    f.$('talk').dispatchEvent(new Event('submit', { cancelable: true }))
    await until('the post reaches K2', () => f.heard.some((m) => m.verb === 'thread.post'))
    expect(f.heard.filter((m) => m.verb === 'thread.post').map((m) => m.args)).toEqual([['cortana::local', 'hi']])
    expect(f.ready()).toBe(1)
    expect(document.body.textContent).not.toMatch(/Allow|permission/i)
  })

  it('arcade without agents:read: a neutral note, no permission wording', async () => {
    const f = await realFrame({
      html: read('examples/arcade/index.html'),
      run: (k2) => new Function('k2', 'document', read('examples/arcade/arcade.js'))(k2, document),
      caps: [],
    })
    await until('ready', () => f.ready() === 1)
    expect(f.subscribed('agents.subscribe')).toEqual([])
    expect(f.$('empty').hidden).toBe(false)
    expect(f.$('empty').textContent).toBe('The game can’t reach your agents right now.')
  })

  it('hello starter: greets the first agent', async () => {
    const f = await realFrame({
      html: read('examples/hello/index.html'),
      run: (k2) => new Function('k2', 'document', 'setInterval', read('examples/hello/hello.js'))(k2, document, () => 0),
      caps: ['agents:read'],
    })
    await until('the starter subscribes to agents', () => f.subscribed('agents.subscribe').length === 1)
    f.push('agents.subscribe', [row('nora', 0)])
    await until('the greeting', () => f.$('greeting').textContent === 'Hello, Nora.')
    expect(f.ready()).toBe(1)
  })
})
