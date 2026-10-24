// K2's widget kit: the Thread panel library, k2-thread@1 (0.46.3, Garden
// gaps). The shipped file runs byte for byte in a fresh JSDOM per test
// against a fake `k2` (agents/thread verbs, caps by verb). It pins the
// behaviour core both looks share: bubbles (yours right, the agent's left)
// with "author · time"; Enter sends, Shift+Enter doesn't, a refused post
// keeps the text; stick-to-bottom with the "N new messages ↓" pill and no
// jump while scrolled up; the working tab (state, counters, clock, "Sent");
// choice cards; attachments; caps; the two looks and their light/dark
// tokens (the Garden's scheme, never the computer's).
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}

const LIB = resolve(__dirname, '../../zen-lib')
const THREAD = readFileSync(resolve(LIB, 'k2-thread@1.0.0/k2-thread.js'), 'utf8')
const MARKED = readFileSync(resolve(LIB, 'marked@18.0.14/marked.umd.js'), 'utf8')
const PURIFY = readFileSync(resolve(LIB, 'dompurify@3.4.17/purify.min.js'), 'utf8')

const ALL = ['agents.subscribe', 'thread.subscribe', 'thread.post', 'thread.answer']
const AGENT = 'ada::rosson.k2.dev'

type View = { address: string; phase: string; items: unknown[]; turn: { state: string; since: number } | null; note?: string }

function fakeK2(opts: { caps?: string[]; postError?: string } = {}) {
  const posts: Array<[string, string]> = []
  const answers: Array<[string, string, string]> = []
  const threadSubs: Array<{ address: string; cb: (v: View) => void; live: boolean }> = []
  const agentSubs: Array<(rows: unknown[]) => void> = []
  let seq = 100
  const k2 = {
    connected: Promise.resolve(),
    config: {},
    motion: { reduced: true },
    can: (verb: string) => (opts.caps ?? ALL).includes(verb),
    agents: {
      subscribe: (cb: (rows: unknown[]) => void) => {
        agentSubs.push(cb)
        return () => undefined
      },
    },
    thread: {
      subscribe: (address: string, cb: (v: View) => void) => {
        const s = { address, cb, live: true }
        threadSubs.push(s)
        return () => {
          s.live = false
        }
      },
      post: async (address: string, text: string) => {
        if (opts.postError) throw Object.assign(new Error(opts.postError), { code: opts.postError })
        posts.push([address, text])
        seq += 1
        return { seq }
      },
      answer: async (address: string, id: string, answer: string) => {
        answers.push([address, id, answer])
        return null
      },
    },
  }
  return { k2, posts, answers, threadSubs, agentSubs }
}

function boot(f: ReturnType<typeof fakeK2>, opts: { markdown?: boolean; scheme?: 'light' | 'dark' } = {}) {
  const dom = new JSDOM('<!doctype html><html><head></head><body><div id="t" style="height:300px"></div></body></html>', {
    runScripts: 'outside-only',
    pretendToBeVisual: true,
  })
  const win = dom.window as unknown as Window & typeof globalThis & { eval: (s: string) => unknown; k2: unknown; K2Thread: any }
  if (opts.scheme) win.document.documentElement.setAttribute('data-zen-scheme', opts.scheme)
  win.k2 = f.k2
  if (opts.markdown) {
    win.eval(MARKED)
    win.eval(PURIFY)
  }
  win.eval(THREAD)
  const doc = win.document
  const root = doc.getElementById('t') as HTMLElement
  const $ = (sel: string): HTMLElement => {
    const n = doc.querySelector<HTMLElement>(sel)
    if (!n) throw new Error(`no ${sel}`)
    return n
  }
  const type = (text: string): void => {
    const t = $('.k2t-field textarea') as HTMLTextAreaElement
    t.value = text
    t.dispatchEvent(new win.Event('input', { bubbles: true }))
  }
  const key = (k: string, shift = false): void => {
    $('.k2t-field textarea').dispatchEvent(new win.KeyboardEvent('keydown', { key: k, shiftKey: shift, bubbles: true, cancelable: true }))
  }
  /** Give the list a layout jsdom doesn't have: 200px tall, 60px a message. */
  const layout = (): { scroll: (top: number) => void; body: HTMLElement } => {
    const body = $('.k2t-body')
    let top = 0
    Object.defineProperty(body, 'clientHeight', { configurable: true, get: () => 200 })
    Object.defineProperty(body, 'scrollHeight', { configurable: true, get: () => 60 * body.querySelectorAll('.k2t-msg').length + 40 })
    Object.defineProperty(body, 'scrollTop', {
      configurable: true,
      get: () => top,
      set: (v: number) => {
        top = Math.max(0, Math.min(v, Math.max(0, body.scrollHeight - 200)))
      },
    })
    return {
      body,
      scroll: (v: number) => {
        top = v
        body.dispatchEvent(new win.Event('scroll'))
      },
    }
  }
  return { win, doc, root, $, type, key, layout }
}

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0))

function msg(seq: number, body: string, mine = false, extra: Record<string, unknown> = {}) {
  return {
    seq,
    id: `m${seq}`,
    doc: { id: `m${seq}`, kind: 'text', from: mine ? 'rosson' : 'ada', to: null, created_at: 1_790_000_000, body, via: mine ? 'compose' : null, choice: null, ...extra },
  }
}

function view(items: unknown[], turn: View['turn'] = null): View {
  return { address: AGENT, phase: 'open', items, turn }
}

describe('k2-thread@1: mounting', () => {
  it('defines K2Thread once, with both looks, and injects its style once', () => {
    const f = fakeK2()
    const { win, doc } = boot(f)
    expect(win.K2Thread.version).toBe('1.0.0')
    expect([...win.K2Thread.looks]).toEqual(['chamfer', 'glass'])
    win.eval(THREAD)
    expect(doc.querySelectorAll('style[data-k2-lib="k2-thread"]')).toHaveLength(1)
  })

  it('refuses a look it doesn’t have, and a missing element, loudly', () => {
    const f = fakeK2()
    const { win, root } = boot(f)
    expect(() => win.K2Thread.mount(root, { look: 'neon' })).toThrow(/chamfer.*glass/)
    expect(() => win.K2Thread.mount(null, {})).toThrow(/element/)
  })

  it('chamfer by default; setLook switches the look in place', () => {
    const f = fakeK2()
    const { win, root, $ } = boot(f)
    const p = win.K2Thread.mount(root, { address: AGENT, label: 'Ada' })
    expect($('.k2t').className).toContain('k2t-look-chamfer')
    expect($('.k2t').hasAttribute('data-k2t-ch')).toBe(true)
    p.setLook('glass')
    expect($('.k2t').className).toContain('k2t-look-glass')
    expect($('.k2t').className).not.toContain('k2t-look-chamfer')
    expect($('.k2t').hasAttribute('data-k2t-ch')).toBe(false)
    expect(p.state().look).toBe('glass')
    expect(() => p.setLook('x')).toThrow()
  })

  it('opens the Thread named in options, with the placeholder; nothing named asks to pick', () => {
    const f = fakeK2()
    const a = boot(f)
    a.win.K2Thread.mount(a.root, { address: AGENT, label: 'Ada' })
    expect(f.threadSubs.map((s) => s.address)).toEqual([AGENT])
    expect((a.$('.k2t-field textarea') as HTMLTextAreaElement).placeholder).toBe('Message Ada')
    expect(a.$('.k2t-compose').hidden).toBe(false)
    const g = fakeK2()
    const b = boot(g)
    b.win.K2Thread.mount(b.root, {})
    expect(g.threadSubs).toHaveLength(0)
    expect(b.$('.k2t-body').textContent).toContain('Pick an agent')
    expect(b.$('.k2t-compose').hidden).toBe(true)
    expect(() => b.win.K2Thread.mount(b.root, {}).open('ada')).toThrow(/handle::host/)
  })
})

describe('k2-thread@1: messages', () => {
  it('draws yours right and the agent’s left, with author · time; agent text as text', async () => {
    const f = fakeK2()
    const { win, root, doc } = boot(f)
    win.K2Thread.mount(root, { address: AGENT, label: 'Ada' })
    f.agentSubs[0]([{ address: AGENT, label: 'Ada Lovelace' }])
    f.threadSubs[0].cb(view([msg(1, 'hi <b>there</b>'), msg(2, 'hello', true)]))
    const msgs = [...doc.querySelectorAll('.k2t-msg')]
    expect(msgs.map((m) => m.className)).toEqual(['k2t-msg k2t-theirs', 'k2t-msg k2t-mine'])
    expect(msgs[0].querySelector('.k2t-bubble-agent')?.textContent).toBe('hi <b>there</b>')
    expect(msgs[0].querySelector('b')).toBeNull()
    expect(msgs[0].querySelector('.k2t-author')?.textContent).toBe('Ada Lovelace')
    expect(msgs[1].querySelector('.k2t-author')?.textContent).toBe('You')
    expect(msgs[1].querySelector('.k2t-time')?.textContent).toMatch(/ago|just now/)
    expect(msgs.every((m) => m.querySelector('.k2t-bubble')?.hasAttribute('data-k2t-ch'))).toBe(true)
    await tick()
  })

  it('renders markdown through DOMPurify when marked and dompurify are loaded', () => {
    const f = fakeK2()
    const { win, root, doc } = boot(f, { markdown: true })
    win.K2Thread.mount(root, { address: AGENT })
    f.threadSubs[0].cb(view([msg(1, '**bold** <img src=x onerror="alert(1)">')]))
    const b = doc.querySelector('.k2t-md') as HTMLElement
    expect(b.querySelector('strong')?.textContent).toBe('bold')
    expect(b.innerHTML).not.toContain('onerror')
  })

  it('patches the list, never rebuilds it: an unchanged message keeps its node', () => {
    const f = fakeK2()
    const { win, root, doc } = boot(f)
    win.K2Thread.mount(root, { address: AGENT })
    f.threadSubs[0].cb(view([msg(1, 'a')]))
    const first = doc.querySelector('.k2t-msg')
    f.threadSubs[0].cb(view([msg(1, 'a'), msg(2, 'b')]))
    expect(doc.querySelector('.k2t-msg')).toBe(first)
    expect(doc.querySelectorAll('.k2t-msg')).toHaveLength(2)
  })

  it('a choice card answers with a tap; secrets say to use K2’s own Thread', () => {
    const f = fakeK2()
    const { win, root, doc } = boot(f)
    win.K2Thread.mount(root, { address: AGENT })
    const choice = msg(3, '', false, { kind: 'choice', choice: { prompt: 'Ship it?', options: [{ label: 'Yes' }, { label: 'No' }], allow_custom: false, status: 'pending' } })
    f.threadSubs[0].cb(view([choice, { seq: 4, id: 's4', doc: { id: 's4', kind: 'secret' } }]))
    const chips = [...doc.querySelectorAll<HTMLButtonElement>('.k2t-opts .k2t-chip')]
    expect(chips.map((c) => c.textContent)).toEqual(['Yes', 'No'])
    chips[0].click()
    expect(f.answers).toEqual([[AGENT, 'm3', 'Yes']])
    expect(doc.querySelector('[data-kind="secret"]')?.textContent).toContain('K2’s own Thread')
  })

  it('0.46.3: an answered settings card shows what K2 did under the answer; other cards look as before', () => {
    const f = fakeK2()
    const { win, root, doc } = boot(f)
    win.K2Thread.mount(root, { address: AGENT })
    const answered = (seq: number, extra: Record<string, unknown>) =>
      msg(seq, '', false, {
        kind: 'choice',
        choice: { prompt: 'Keep it?', options: [{ label: 'Keep as mine' }, { label: 'Leave it' }], allow_custom: false, status: 'answered', answer: 'Keep as mine', ...extra },
      })
    f.threadSubs[0].cb(view([answered(3, { resultMessage: 'Kept as yours. Undo in Settings → Gardens → Local Data.' }), answered(4, {})]))
    const results = [...doc.querySelectorAll('.k2t-result')]
    expect(results.map((r) => r.textContent)).toEqual(['Kept as yours. Undo in Settings → Gardens → Local Data.'])
    const plain = doc.querySelector('[data-seq="4"]') ?? [...doc.querySelectorAll('.k2t-status')][1]?.parentElement
    expect(plain?.querySelector('.k2t-result')).toBeNull()
    expect([...doc.querySelectorAll('.k2t-status')].map((s) => s.textContent)).toEqual(['Answered: Keep as mine', 'Answered: Keep as mine'])
  })

  it('attachments: a data: image inline, other files as chips; onOpenFile makes them buttons', () => {
    const f = fakeK2()
    const { win, root, doc } = boot(f)
    const opened: string[] = []
    win.K2Thread.mount(root, { address: AGENT, onOpenFile: (file: { name: string }) => opened.push(file.name) })
    const png = 'data:image/png;base64,iVBORw0KGgo='
    f.threadSubs[0].cb(
      view([
        msg(5, '', false, { attachments: [{ name: 'shot.png', mime: 'image/png', preview: png }, { name: 'report.pdf', path: '/tmp/report.pdf' }] }),
        msg(6, 'see https://example.com', false, { attachments: [{ name: 'evil.png', url: 'https://example.com/x.png' }] }),
      ]),
    )
    const first = doc.querySelector('[data-seq="5"]') as HTMLElement
    expect(first.querySelector('.k2t-bubble')).toBeNull()
    expect((first.querySelector('img.k2t-img') as HTMLImageElement).src).toBe(png)
    const chip = first.querySelector<HTMLButtonElement>('button.k2t-file')!
    expect(chip.textContent).toBe('report.pdf')
    expect(chip.title).toBe('/tmp/report.pdf')
    chip.click()
    expect(opened).toEqual(['report.pdf'])
    // A web image is never loaded (the frame has no network): a chip.
    expect(doc.querySelector('[data-seq="6"] img')).toBeNull()
    expect(doc.querySelector('[data-seq="6"] .k2t-file')?.textContent).toBe('evil.png')
  })
})

describe('k2-thread@1: the message box', () => {
  it('Enter sends, Shift+Enter doesn’t; the text clears once K2 took it; Send lights only with text', async () => {
    const f = fakeK2()
    const { win, root, $, type, key } = boot(f)
    win.K2Thread.mount(root, { address: AGENT, label: 'Ada' })
    const send = $('.k2t-send') as HTMLButtonElement
    expect(send.disabled).toBe(true)
    type('line one')
    expect(send.disabled).toBe(false)
    expect(send.className).toContain('k2t-ready')
    key('Enter', true)
    await tick()
    expect(f.posts).toEqual([])
    key('Enter')
    await tick()
    await tick()
    expect(f.posts).toEqual([[AGENT, 'line one']])
    expect(($('.k2t-field textarea') as HTMLTextAreaElement).value).toBe('')
    expect(send.disabled).toBe(true)
  })

  it('an empty box measured before layout (scrollHeight 0) is one whole line tall in each look, never clipped', () => {
    // jsdom has no layout: scrollHeight is 0, as for a box measured while
    // hidden. The floor is line-height + padding (border-box): glass 22+8,
    // chamfer 20+12. Glass's used to be 22 and clipped its placeholder.
    for (const [look, px] of [
      ['glass', '30px'],
      ['chamfer', '32px'],
    ] as const) {
      const { win, root, $ } = boot(fakeK2())
      win.K2Thread.mount(root, { address: AGENT, label: 'Ada', look })
      expect(($('.k2t-field textarea') as HTMLTextAreaElement).style.height, look).toBe(px)
    }
  })

  it('a refused post keeps the text and says why', async () => {
    const f = fakeK2({ postError: 'sending_off' })
    const { win, root, $, type, key } = boot(f)
    const errs: unknown[] = []
    win.K2Thread.mount(root, { address: AGENT, onError: (e: unknown) => errs.push(e) })
    type('keep me')
    key('Enter')
    await tick()
    await tick()
    expect(($('.k2t-field textarea') as HTMLTextAreaElement).value).toBe('keep me')
    expect($('.k2t-err').hidden).toBe(false)
    expect($('.k2t-err').textContent).toContain('posting is paused')
    expect(errs).toHaveLength(1)
  })

  it('panel.post sends one at a time and refuses empty text or no Thread', async () => {
    const f = fakeK2()
    const { win, root } = boot(f)
    const p = win.K2Thread.mount(root, {})
    await expect(p.post('x')).rejects.toThrow(/no Thread/)
    p.open(AGENT, 'Ada')
    await expect(p.post('   ')).rejects.toThrow(/nothing/)
    const a = p.post('one')
    await expect(p.post('two')).rejects.toThrow(/on its way/)
    await a
    expect(f.posts).toEqual([[AGENT, 'one']])
  })

  it('caps: no thread:post hides the box; no thread:read says so', () => {
    const f = fakeK2({ caps: ['thread.subscribe'] })
    const a = boot(f)
    a.win.K2Thread.mount(a.root, { address: AGENT })
    expect(a.$('.k2t-compose').hidden).toBe(true)
    const g = fakeK2({ caps: ['thread.post'] })
    const b = boot(g)
    b.win.K2Thread.mount(b.root, { address: AGENT })
    expect(g.threadSubs).toHaveLength(0)
    expect(b.$('.k2t-body').textContent).toContain('thread:read')
  })
})

describe('k2-thread@1: stick-to-bottom and the new-messages pill', () => {
  it('pinned: a new message keeps it at the bottom; scrolled up: nothing moves and the pill counts', () => {
    const f = fakeK2()
    const { win, root, $, layout } = boot(f)
    win.K2Thread.mount(root, { address: AGENT })
    const cb = f.threadSubs[0].cb
    const items = [msg(1, 'a'), msg(2, 'b'), msg(3, 'c'), msg(4, 'd'), msg(5, 'e')]
    cb(view(items.slice(0, 1)))
    const { body, scroll } = layout()
    cb(view(items.slice(0, 4)))
    expect(body.scrollTop).toBe(60 * 4 + 40 - 200)
    const pill = $('.k2t-newer') as HTMLButtonElement
    expect(pill.hidden).toBe(true)
    // The person scrolls up.
    scroll(0)
    scroll(0)
    cb(view(items))
    expect(body.scrollTop).toBe(0)
    expect(pill.hidden).toBe(false)
    expect(pill.textContent).toBe('1 new message ↓')
    cb(view([...items, msg(6, 'f')]))
    expect(pill.textContent).toBe('2 new messages ↓')
    // Your own post coming back never counts... and goes to the bottom.
    cb(view([...items, msg(6, 'f'), msg(7, 'mine', true)]))
    expect(pill.textContent).toBe('2 new messages ↓')
    expect(body.scrollTop).toBe(0)
    pill.click()
    expect(pill.hidden).toBe(true)
    expect(body.scrollTop).toBe(60 * 7 + 40 - 200)
  })

  it('scrolling back down by hand clears the pill', () => {
    const f = fakeK2()
    const { win, root, $, layout } = boot(f)
    win.K2Thread.mount(root, { address: AGENT })
    const cb = f.threadSubs[0].cb
    cb(view([msg(1, 'a')]))
    const { scroll } = layout()
    cb(view([msg(1, 'a'), msg(2, 'b'), msg(3, 'c'), msg(4, 'd')]))
    scroll(0)
    scroll(0)
    cb(view([msg(1, 'a'), msg(2, 'b'), msg(3, 'c'), msg(4, 'd'), msg(5, 'e')]))
    expect($('.k2t-newer').hidden).toBe(false)
    scroll(60 * 5 + 40 - 200)
    expect($('.k2t-newer').hidden).toBe(true)
  })

  it('opening another Thread starts at its bottom with no pill', () => {
    const f = fakeK2()
    const { win, root, $ } = boot(f)
    const p = win.K2Thread.mount(root, { address: AGENT })
    p.open('bob::rosson.k2.dev', 'Bob')
    expect(f.threadSubs[0].live).toBe(false)
    expect(f.threadSubs[1].address).toBe('bob::rosson.k2.dev')
    // A late view for the old Thread is ignored.
    f.threadSubs[0].cb(view([msg(1, 'stale')]))
    expect($('.k2t-body').textContent).not.toContain('stale')
    expect(p.state()).toMatchObject({ address: 'bob::rosson.k2.dev', stuck: true, unseen: 0 })
  })
})

describe('k2-thread@1: the working tab', () => {
  it('shows the state, counters and clock while the agent works; "Sent" after a post', async () => {
    const f = fakeK2()
    const { win, root, $, type, key } = boot(f)
    const p = win.K2Thread.mount(root, { address: AGENT })
    const ear = $('.k2t-ear')
    expect(ear.hidden).toBe(true)
    type('go')
    key('Enter')
    expect(ear.hidden).toBe(false)
    expect(ear.getAttribute('data-tone')).toBe('pending')
    expect(ear.textContent).toContain('Sent · waiting…')
    await tick()
    f.agentSubs[0]([{ address: AGENT, label: 'Ada', counts: { subagents: 2, tools: 1, commands: 0 } }])
    f.threadSubs[0].cb(view([msg(1, 'on it')], { state: 'working', since: Date.now() - 65_000 }))
    expect(ear.getAttribute('data-tone')).toBe('working')
    expect(ear.querySelector('.k2t-ear-label')?.textContent).toBe('Working')
    expect([...ear.querySelectorAll('[data-count]')].map((t) => [t.getAttribute('data-count'), t.textContent])).toEqual([
      ['subagents', '2'],
      ['tools', '1'],
    ])
    expect(ear.querySelector('.k2t-ear-clock')?.textContent).toMatch(/^1:0[5-6]$/)
    expect(ear.getAttribute('aria-label')).toContain('2 subagents')
    expect(p.state().tone).toBe('working')
    f.threadSubs[0].cb(view([msg(1, 'on it')], { state: 'needs-you', since: Date.now() }))
    expect(ear.getAttribute('data-tone')).toBe('needs-you')
    f.threadSubs[0].cb(view([msg(1, 'done')], null))
    f.agentSubs[0]([{ address: AGENT, label: 'Ada' }])
    expect(ear.hidden).toBe(true)
    p.destroy()
    expect(f.threadSubs[0].live).toBe(false)
    expect(root.querySelector('.k2t')).toBeNull()
  })
})

describe('k2-thread@1: looks and theme', () => {
  const css = (): string => {
    const f = fakeK2()
    const { doc } = boot(f)
    return doc.querySelector('style[data-k2-lib="k2-thread"]')!.textContent ?? ''
  }

  it('both looks have light tokens and dark ones keyed on the Garden’s scheme, never the computer’s', () => {
    const c = css()
    expect(c).toContain('.k2t-look-chamfer{--k2t-bg:var(--zen-surface,#ffffff)')
    expect(c).toContain(':root[data-zen-scheme="dark"] .k2t-look-chamfer{--k2t-bg:var(--zen-surface,#1a1b1f)')
    expect(c).toContain('.k2t-look-glass{--k2t-bg:#ffffff')
    expect(c).toContain(':root[data-zen-scheme="dark"] .k2t-look-glass{--k2t-bg:#0a0a0a')
    expect(c).not.toContain('prefers-color-scheme')
  })

  it('chamfer: K2 Thread’s cuts (bubbles 4px with the sender’s corner sharp, field 4, Send 5, tab 5 on top)', () => {
    const c = css()
    expect(c).toContain('.k2t-look-chamfer .k2t-bubble-me{--ch-tl:4px;--ch-tr:4px;--ch-br:0px;--ch-bl:4px;')
    expect(c).toContain('.k2t-look-chamfer .k2t-bubble-agent{--ch-tl:4px;--ch-tr:4px;--ch-br:4px;--ch-bl:0px;')
    expect(c).toContain('.k2t-look-chamfer .k2t-field{--ch-tl:4px;--ch-tr:4px;--ch-br:4px;--ch-bl:4px;')
    expect(c).toContain('.k2t-look-chamfer .k2t-send{--ch-tl:5px;--ch-tr:5px;--ch-br:5px;--ch-bl:5px;')
    expect(c).toContain('.k2t-look-chamfer .k2t-ear{--ch-tl:5px;--ch-tr:5px;--ch-br:0px;--ch-bl:0px;')
    expect(c).toContain('.k2t-look-chamfer .k2t-bubble{padding:9px 14px}')
  })

  it('glass: Companion’s round shapes and two blur layers; a narrow panel uses the whole width', () => {
    const c = css()
    expect(c).toContain('.k2t-look-glass .k2t-bubble{padding:8px 13px;line-height:1.4;border-radius:18px}')
    expect(c).toContain('.k2t-look-glass .k2t-field{padding:4px 4px 4px 14px;border-radius:20px')
    expect(c.match(/-webkit-backdrop-filter:blur\(/g)).toHaveLength(2)
    expect(c).toContain('@container (max-width: 479.98px){.k2t-msg{max-width:calc(100% - 12px)}}')
    expect(c).toContain('prefers-reduced-transparency')
  })
})
