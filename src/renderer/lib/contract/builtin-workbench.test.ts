// 0.46.7 Workbench: the real checked-in k2:work-inbox@1 (and the shared
// workbench-kit) in a JSDOM with a fake k2: sections by what needs you,
// pin / snooze / hide kept in the widget's data, a snoozed task coming back
// at its time ("Woke"), other computers' agents read only, and picking a
// task showing its Thread and telling the Review column.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: Win }
}

interface Win {
  document: Document
  eval(code: string): unknown
  Date: DateConstructor
  k2: unknown
  WB: Record<string, (...args: unknown[]) => unknown> & { LABEL: Record<string, string> }
  HTMLElement: typeof HTMLElement
  Event: typeof Event
}

const WIDGETS = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets')
const read = (p: string): string => readFileSync(resolve(WIDGETS, p), 'utf8')

const HOUR = 3600e3
const T0 = Date.UTC(2026, 9, 29, 15, 0, 0)

function tasks(): unknown[] {
  return [
    { address: 'k2/fix::local', thread: 'k2/fix', workspace: 'k2', workspaceLabel: 'K2', name: 'fix', harness: 'claude', live: true, worktree: '.worktrees/k2-fix', branch: 'k2/fix', base: 'main', head: 'abc1234', ahead: 1, behind: 0, dirty: 0, taskState: null, landedAt: null, retiredAt: null },
    { address: 'k2/docs::local', thread: 'k2/docs', workspace: 'k2', workspaceLabel: 'K2', name: 'docs', harness: 'codex', live: false, worktree: null, branch: null, base: null, head: null, ahead: null, behind: null, dirty: null, taskState: null, landedAt: null, retiredAt: null },
    { address: 'k2/ship::local', thread: 'k2/ship', workspace: 'k2', workspaceLabel: 'K2', name: 'ship', harness: 'claude', live: true, worktree: '.worktrees/k2-ship', branch: 'k2/ship', base: 'main', head: 'def5678', ahead: 3, behind: 0, dirty: 0, taskState: null, landedAt: null, retiredAt: null },
  ]
}

async function inbox() {
  const html = read('work-inbox-1/index.html').replace(/<script[^>]*><\/script>/g, '')
  const dom = new JSDOM(html, { runScripts: 'outside-only', pretendToBeVisual: true })
  const win = dom.window
  let now = T0
  win.Date.now = () => now
  const store = new Map<string, unknown>()
  const subs: Array<(c: unknown) => void> = []
  const shown: unknown[] = []
  const opened: string[] = []
  const activity = new Map<string, (v: unknown) => void>()
  const k2 = {
    can: (v: string) => v !== 'agents.openTerminal',
    ready: () => undefined,
    work: { list: async () => tasks() },
    agents: {
      list: async () => [
        { address: 'k2::local', label: 'K2' },
        { address: 'k2/fix::local', label: 'fix · K2' },
        { address: 'ops::box.k2.dev', label: 'Ops', server: 'box', working: true, needsYou: false },
      ],
      show: async (list: unknown) => void shown.push(list),
      stop: async () => ({ stopped: true }),
      start: async () => ({ address: 'k2/new::local', thread: 'k2/new', branch: 'k2/new', worktree: '.worktrees/k2-new' }),
    },
    conversation: { open: async (a: string) => void opened.push(a) },
    thread: {
      unread: async () => ({ total: 3, waitingTotal: 1, byConversation: [{ id: 'c1', addr: 'k2/fix', lastReadSeq: 1, lastSeq: 3, unread: 2, waiting: 1 }] }),
      onActivity: (a: string, cb: (v: unknown) => void) => {
        activity.set(a, cb)
        return () => activity.delete(a)
      },
    },
    store: {
      get: async (k: string) => (store.has(k) ? { key: k, rev: 1, value: store.get(k), updatedAt: 0, by: 'you' } : null),
      put: async (k: string, v: unknown) => {
        store.set(k, v)
        return { rev: 1 }
      },
      list: async ({ prefix }: { prefix: string }) => ({
        items: [...store.entries()].filter(([k]) => k.startsWith(prefix)).map(([key, value]) => ({ key, rev: 1, value, updatedAt: 0, by: 'you' })),
        next: null,
      }),
      subscribe: (_p: string, cb: (c: unknown) => void) => {
        subs.push(cb)
        return () => undefined
      },
    },
  }
  win.k2 = k2
  win.eval(read('workbench-kit/work-common.js'))
  win.eval(read('work-inbox-1/inbox.js'))
  const settle = async () => {
    for (let i = 0; i < 10; i++) await new Promise((r) => setTimeout(r, 0))
  }
  await settle()
  const doc = win.document
  const sections = () =>
    [...doc.querySelectorAll('.section')].map((s) => ({
      title: s.querySelector('h4')?.firstChild?.textContent ?? '',
      rows: [...s.querySelectorAll('.row .name')].map((n) => n.textContent),
      labels: [...s.querySelectorAll('.row .label')].map((n) => n.textContent),
    }))
  const redraw = () => doc.getElementById('filter')!.dispatchEvent(new win.Event('input'))
  const rowNamed = (name: string) => [...doc.querySelectorAll('.row')].find((r) => r.querySelector('.name')?.textContent === name) as HTMLElement
  const click = (el: Element) => (el as HTMLElement).click()
  const button = (row: HTMLElement, label: string) => [...row.querySelectorAll('button')].find((b) => b.textContent === label) as HTMLElement
  return { win, doc, store, shown, opened, activity, sections, redraw, rowNamed, click, button, settle, setNow: (t: number) => (now = t) }
}

describe('k2:work-inbox@1', () => {
  it('sorts tasks by what needs you, and lists other computers read only', async () => {
    const t = await inbox()
    expect(t.sections()).toEqual([
      { title: 'Needs you (1)', rows: ['fix'], labels: ['Needs you'] },
      { title: 'Ready to land (1)', rows: ['ship'], labels: ['Ready to land'] },
      { title: 'Idle (1)', rows: ['docs'], labels: ['Idle'] },
      { title: 'Other computers (read only) (1)', rows: ['Ops'], labels: ['Working'] },
    ])
    // A working turn moves a task to Working.
    t.activity.get('k2/ship::local')!({ address: 'k2/ship::local', live: true, turn: { state: 'working' } })
    expect(t.sections()[1]).toEqual({ title: 'Working (1)', rows: ['ship'], labels: ['Working'] })
    // A remote row has no Stop and no Start.
    expect(t.button(t.rowNamed('Ops'), 'Stop')).toBeUndefined()
  })

  it('picking a task shows its Thread and tells the Review column', async () => {
    const t = await inbox()
    t.click(t.rowNamed('ship'))
    await t.settle()
    expect(t.shown).toContainEqual(['k2/ship::local'])
    expect(t.opened).toEqual(['k2/ship::local'])
    expect(t.store.get('sel')).toMatchObject({ address: 'k2/ship::local', thread: 'k2/ship', branch: 'k2/ship', base: 'main', remote: false })
  })

  it('pin and hide are kept in the widget’s data', async () => {
    const t = await inbox()
    t.click(t.button(t.rowNamed('docs'), 'Pin'))
    expect(t.store.get('item/k2/docs')).toMatchObject({ pinned: true })
    expect(t.sections()[0]).toEqual({ title: 'Pinned (1)', rows: ['docs'], labels: ['Idle'] })
    t.click(t.button(t.rowNamed('ship'), 'Hide'))
    expect(t.store.get('item/k2/ship')).toMatchObject({ hidden: true })
    expect(t.sections().map((s) => s.title)).not.toContain('Hidden (1)')
    t.click(t.doc.getElementById('btnHidden')!)
    expect(t.sections().find((s) => s.title === 'Hidden (1)')?.rows).toEqual(['ship'])
  })

  it('a snoozed task leaves its section and comes back at its time as Woke', async () => {
    const t = await inbox()
    t.click(t.button(t.rowNamed('fix'), 'Snooze…'))
    const menu = [...t.doc.querySelectorAll('.menu button')]
    expect(menu.map((b) => b.textContent)).toEqual(['For 1 hour', 'For 4 hours', 'Until tomorrow 9:00', 'Until Monday 9:00'])
    t.click(menu[0])
    expect(t.store.get('item/k2/fix')).toMatchObject({ snoozeUntil: T0 + HOUR })
    const snoozed = t.sections().find((s) => s.title === 'Snoozed (1)')
    expect(snoozed?.rows).toEqual([]) // folded by default
    t.setNow(T0 + HOUR + 1000)
    t.redraw()
    expect(t.sections()[0]).toEqual({ title: 'Needs you (1)', rows: ['fix'], labels: ['Woke'] })
    // Picking it clears the snooze.
    t.click(t.rowNamed('fix'))
    await t.settle()
    expect((t.store.get('item/k2/fix') as Record<string, unknown>).snoozeUntil).toBeUndefined()
  })

  it('the shared rules: taskState and sections', async () => {
    const { win } = await inbox()
    const WB = win.WB
    expect(WB.taskState({ worktree: '.w', ahead: 0, dirty: 2 }, {})).toBe('ready')
    expect(WB.taskState({ worktree: null, ahead: 5 }, {})).toBe('idle')
    expect(WB.taskState({ taskState: 'landed' }, { waiting: 3 })).toBe('landed')
    expect(WB.taskState({}, { turn: 'needs-you' })).toBe('needs-you')
    expect(WB.section('needs-you', { hidden: true }, T0)).toBe('hidden')
    expect(WB.section('working', { snoozeUntil: T0 + 1 }, T0)).toBe('snoozed')
    expect(WB.section('working', { snoozeUntil: T0 - 1 }, T0)).toBe('working')
    expect(WB.section('ready', { settled: true }, T0)).toBe('settled')
    expect(WB.itemKey('K2/Fix Me')).toBe('item/k2/fix-me')
    expect(WB.isSidecar('k2/fix::local')).toBe(true)
    expect(WB.remote('ops::box.k2.dev')).toBe(true)
  })

  it('never builds markup from data', () => {
    for (const f of ['workbench-kit/work-common.js', 'work-inbox-1/inbox.js', 'review-1/review.js']) {
      const js = read(f)
      for (const bad of ['innerHTML', 'outerHTML', 'insertAdjacentHTML', 'document.write']) expect(js, `${f}: ${bad}`).not.toContain(bad)
    }
  })
})
