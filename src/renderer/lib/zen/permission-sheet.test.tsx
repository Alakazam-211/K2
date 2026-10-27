// @vitest-environment jsdom
// Permission cards v1 in a Garden (prd-agent-settings-cards-v1 §8): the
// widget's copy of the card (K2's words, a closed shape), the kit drawing
// it with one button, the bridge turning that button's `thread.answer` into
// K2's sheet outside the frame (a widget never answers), and the sheet
// answering on the Thread's server as the person.
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const daemonCliPost = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => ({ ok: true })))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
  daemonCliGet: () => Promise.reject(new Error('permission-sheet.test: no GET here')),
}))

import type { ServerScope } from '@/kessel/server-scope'
import { threadItemsFromSnapshot, type OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { APPROVED, batchDoc, permissionDoc, VIEWERS } from '@/components/SessionView/permissionCard.fixtures'
import { ZenPermissionSheet } from '@/components/Zen/widgets/ZenPermissionSheet'
import type { ZenWidgetBridge } from './zen-bridge'
import { createZenCustomLayer, type ZenCustomLayerDeps } from './zen-custom-bridge'
import type { ZenCustomWidgetPayload, ZenFrameReply } from './zen-custom-types'
import { projectZenThreadItem } from './zen-custom-projection'
import { createZenPermissionSheet, projectZenPermission, type ZenPermissionAsk } from './permission-sheet'

afterEach(() => {
  cleanup()
  daemonCliPost.mockClear()
})

const NOW = 1761062000

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 412, id: doc.id, doc }] }).items[0]
}

const permissionOf = (doc: Record<string, unknown>) => {
  const p = itemOf(doc).doc.choice?.permission
  if (!p) throw new Error('not a permission card')
  return p
}

describe('the widget’s copy of the card (closed shape, K2’s words)', () => {
  it('an approver: the headline, the agent’s words, one button; never values, notes or names', () => {
    const w = projectZenPermission(permissionOf(permissionDoc({})), 'billing', NOW)
    expect(w).toEqual({
      kind: 'permission',
      state: 'pending',
      mode: 'approve',
      headline: "Let billing run jobs on this server's compute nodes.",
      setting: 'Allow compute nodes',
      direction: 'loosen',
      duration: 'until someone turns it off',
      why: 'billing says: Run the test suite on k2n2-m1; it takes 40 min here',
      line: null,
      button: 'Review and answer',
      count: 1,
    })
    const wire = JSON.stringify(w)
    for (const leak of ['dana', 'maya', 'payroll', 'pr_7f3a2c', 'workspaces/billing/agent', 'workspace: billing']) {
      expect([leak, wire.includes(leak)]).toEqual([leak, false])
    }
  })

  it('the asker: the waiting line, and a note button only when the daemon allows one', () => {
    const asker = projectZenPermission(
      permissionOf(permissionDoc({ viewer: VIEWERS.dana, view: { noteBox: { allowed: true, max: 500, mine: null } } })),
      'billing',
      NOW,
    )
    expect([asker.mode, asker.line, asker.button]).toEqual([
      'wait',
      'Asked an admin to turn on Allow compute nodes for you. You can keep working with billing meanwhile.',
      'Leave a note',
    ])
    expect(projectZenPermission(permissionOf(permissionDoc({ viewer: VIEWERS.lee })), 'billing', NOW).button).toBeNull()
  })

  it('closed states: the state line; Review only while Turn off again is open', () => {
    const approved = projectZenPermission(permissionOf(permissionDoc(APPROVED)), 'billing', NOW)
    expect(approved.line).toMatch(/^Allowed by you, from the phone, \d\d:\d\d$/)
    expect(approved.button).toBe('Review')
    expect(projectZenPermission(permissionOf(permissionDoc(APPROVED)), 'billing', 1761666001).button).toBeNull()
    const expired = projectZenPermission(permissionOf(permissionDoc({ view: { state: 'expired' } })), 'billing', NOW)
    expect([expired.line, expired.button]).toEqual(['Expired unanswered', null])
  })

  it('a batch: how many, and one button for the approver', () => {
    const b = projectZenPermission(permissionOf(batchDoc()), 'billing', NOW)
    expect([b.kind, b.headline, b.count, b.button]).toEqual(['batch', 'billing asks for 3 permissions.', 3, 'Review and answer'])
  })

  it('rides the widget’s Thread item as choice.permission; other cards have none', () => {
    const p = projectZenThreadItem(itemOf(permissionDoc({})))
    const choice = p && 'choice' in p.doc ? p.doc.choice : null
    expect(choice?.permission?.headline).toBe("Let billing run jobs on this server's compute nodes.")
    const plain = projectZenThreadItem(
      itemOf({ id: 'c1', kind: 'choice', from: 'b', choice: { prompt: 'Ok?', options: [{ label: 'Yes' }], allow_custom: false, status: 'pending' } }),
    )
    expect(plain && 'choice' in plain.doc && plain.doc.choice && 'permission' in plain.doc.choice).toBe(false)
  })
})

describe('the sheet broker', () => {
  const card = itemOf(permissionDoc({}))
  const plain = itemOf({ id: 'c1', kind: 'choice', from: 'b', choice: { prompt: 'Ok?', options: [], allow_custom: false, status: 'pending' } })

  it('opens only for a permission card, only after a press, one at a time', async () => {
    let pressed = false
    let close: () => void = () => undefined
    const shown: ZenPermissionAsk[] = []
    const sheet = createZenPermissionSheet({
      find: (_a, id) => (id === card.id ? card : id === plain.id ? plain : null),
      show: (ask) =>
        new Promise<void>((r) => {
          shown.push(ask)
          close = r
        }),
      gesture: () => pressed,
    })
    expect([sheet.handles('billing', card.id), sheet.handles('billing', plain.id)]).toEqual([true, false])
    expect(await sheet.open('billing', card.id)).toMatchObject({ ok: false, code: 'failed' })
    pressed = true
    const first = sheet.open('billing', card.id)
    expect(await sheet.open('billing', card.id)).toMatchObject({ ok: false, code: 'rate_limited' })
    close()
    expect(await first).toEqual({ ok: true })
    expect(shown).toEqual([{ address: 'billing', cardId: card.id }])
  })
})

function payload(): ZenCustomWidgetPayload {
  return {
    id: 'thread',
    kind: 'custom',
    widget: 'thread-panel',
    column: 0,
    props: { config: {} },
    caps: ['agents:read', 'thread:read', 'thread:post'],
    requested: ['agents:read', 'thread:read', 'thread:post'],
    source: 'user',
    name: 'Thread',
    description: null,
    reasons: {},
    libs: [],
    hash: 'h1',
    state: 'ok',
    errors: [],
    warnings: [],
    origin: 'local',
    paused: null,
  } as ZenCustomWidgetPayload
}

describe('the bridge: thread.answer on a permission card opens K2’s sheet, never answers', () => {
  function layerWith(permissions: ZenCustomLayerDeps['permissions']) {
    const calls: string[] = []
    const inner = {
      widgetId: 'thread',
      caps: new Set<string>(),
      call: (verb: string) => {
        calls.push(verb)
        return null
      },
    } as unknown as ZenWidgetBridge
    const row = { address: 'billing::local', label: 'billing', index: 0, hostKey: 'local', openable: true } as never
    const deps: ZenCustomLayerDeps = {
      widget: payload,
      gardenId: () => 'g-test0001',
      inner,
      boundRows: () => [row],
      ensureConversation: async () => undefined,
      push: () => undefined,
      stop: () => undefined,
      paused: () => false,
      pauseForRunaway: () => undefined,
      theme: () => ({}),
      onThemeChange: () => () => undefined,
      now: () => 1_000_000,
      ...(permissions ? { permissions } : {}),
    }
    return { layer: createZenCustomLayer(deps), calls }
  }
  const ask = async (layer: ReturnType<typeof createZenCustomLayer>, id: string): Promise<ZenFrameReply> => {
    const r = await layer.handle({ id: 1, verb: 'thread.answer', args: ['billing::local', id, 'Review and answer'] })
    if (!r) throw new Error('no reply')
    return r
  }

  it('the sheet opens and the widget hears null; nothing reaches the daemon', async () => {
    const open = vi.fn(async () => ({ ok: true as const }))
    const { layer, calls } = layerWith({ handles: (_a, id) => id === 'tc_7f3a2c90', open })
    expect(await ask(layer, 'tc_7f3a2c90')).toEqual({ id: 1, ok: true, value: null })
    expect(open).toHaveBeenCalledWith('billing::local', 'tc_7f3a2c90')
    expect(calls).toEqual([])
    // Any other card answers as before.
    expect(await ask(layer, 'c1')).toMatchObject({ ok: true })
    expect(calls).toEqual(['thread.answer'])
  })

  it('a refused open reaches the widget with its code', async () => {
    const { layer } = layerWith({
      handles: () => true,
      open: async () => ({ ok: false, code: 'rate_limited', message: 'K2 is already showing a permission card.' }),
    })
    const r = await ask(layer, 'tc_7f3a2c90')
    expect('ok' in r && !r.ok && r.error.code).toBe('rate_limited')
  })
})

describe('K2’s sheet outside the frame', () => {
  const scope = { serverSupports: () => false } as unknown as ServerScope

  it('draws the live card and answers on the Thread’s server as the person', async () => {
    let item = itemOf(permissionDoc({}))
    let notify: () => void = () => undefined
    const onDone = vi.fn()
    render(
      <ZenPermissionSheet
        ask={{ address: 'billing::local', cardId: item.id }}
        onDone={onDone}
        target={() => ({ item, scope, threadAddr: 'billing' })}
        onChange={(_a, cb) => {
          notify = cb
          return () => undefined
        }}
      />,
    )
    expect(screen.getByTestId('zen-permission-sheet').getAttribute('role')).toBe('dialog')
    fireEvent.click(screen.getByTestId('permission-allow'))
    await waitFor(() =>
      expect(daemonCliPost).toHaveBeenCalledWith(scope, 'thread/answer', {
        addr: 'billing',
        id: 'tc_7f3a2c90',
        answer: 'Allow',
        client: 'desktop',
      }),
    )
    // The card's next state lands live.
    item = itemOf(permissionDoc(APPROVED))
    notify()
    await waitFor(() => expect(screen.getByTestId('permission-card').getAttribute('data-state')).toBe('approved'))
    fireEvent.click(screen.getByTestId('zen-permission-sheet-done'))
    expect(onDone).toHaveBeenCalledTimes(1)
  })

  it('says so when the card is gone', () => {
    render(
      <ZenPermissionSheet ask={{ address: 'a', cardId: 'x' }} onDone={() => undefined} target={() => null} onChange={() => () => undefined} />,
    )
    expect(screen.getByTestId('zen-permission-sheet-gone')).toBeTruthy()
  })
})

// ── The kit (k2-thread@1): the shipped file, byte for byte, in a JSDOM ──

const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
  JSDOM: new (html: string, opts: { runScripts: 'outside-only'; pretendToBeVisual: boolean }) => { window: unknown }
}
const THREAD = readFileSync(resolve(__dirname, '../../zen-lib/k2-thread@1.1.0/k2-thread.js'), 'utf8')

describe('the kit draws a permission card with one button that asks K2', () => {
  it('K2’s words, no Allow / Deny chips; the button sends thread.answer (the bridge opens the sheet)', () => {
    const answers: unknown[][] = []
    let push: (v: unknown) => void = () => undefined
    const k2 = {
      connected: Promise.resolve(),
      config: {},
      motion: { reduced: true },
      can: () => true,
      agents: { subscribe: () => () => undefined },
      thread: {
        subscribe: (_a: string, cb: (v: unknown) => void) => {
          push = cb
          return () => undefined
        },
        post: async () => ({ seq: 1 }),
        answer: async (...a: unknown[]) => {
          answers.push(a)
          return null
        },
      },
    }
    const dom = new JSDOM('<!doctype html><html><head></head><body><div id="t"></div></body></html>', {
      runScripts: 'outside-only',
      pretendToBeVisual: true,
    })
    const win = dom.window as unknown as Window & typeof globalThis & { eval: (s: string) => unknown; k2: unknown; K2Thread: any }
    win.k2 = k2
    win.eval(THREAD)
    win.K2Thread.mount(win.document.getElementById('t'), { address: 'billing::local' })
    const item = projectZenThreadItem(itemOf(permissionDoc({})))
    push({ address: 'billing::local', phase: 'ready', items: [item], turn: null })
    const doc = win.document
    const block = doc.querySelector('[data-k2t-permission]') as HTMLElement
    expect(block.getAttribute('data-k2t-permission')).toBe('pending')
    expect(block.querySelector('.k2t-perm-title')?.textContent).toBe("Let billing run jobs on this server's compute nodes.")
    expect(block.querySelector('.k2t-perm-tag')?.textContent).toBe('Loosens access')
    expect(block.querySelector('.k2t-perm-why')?.textContent).toBe('billing says: Run the test suite on k2n2-m1; it takes 40 min here')
    // The fallback prompt isn't drawn, and neither are the raw options.
    expect(doc.body.textContent).not.toContain('billing asks:')
    const chips = [...doc.querySelectorAll<HTMLButtonElement>('.k2t-chip')]
    expect(chips.map((c) => c.textContent)).toEqual(['Review and answer'])
    chips[0].click()
    expect(answers).toEqual([['billing::local', 'tc_7f3a2c90', 'Review and answer']])
  })
})
