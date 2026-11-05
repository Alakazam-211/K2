// @vitest-environment jsdom
// Secret cards in a custom Garden (catalog v50): `thread.answer(address,
// cardId)` on a secret card opens K2's own sheet outside the frame; the
// person types the value there and it goes through the window's own answer
// path with no widget origin. The widget hears `null`, never the value.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'
import { threadItemsFromSnapshot, type OverlayThreadItem, type ThreadAnswerPayload } from '@/components/SessionView/overlayThread'
import { ZenSecretSheet } from '@/components/Zen/widgets/ZenSecretSheet'
import type { ZenWidgetBridge } from './zen-bridge'
import { createZenCustomLayer, type ZenCustomLayerDeps } from './zen-custom-bridge'
import type { ZenCustomWidgetPayload } from './zen-custom-types'
import { createZenSecretSheet, type ZenSecretAsk } from './secret-sheet'

afterEach(() => cleanup())

const VALUE = 'k2test_TESTVALUE_123'

function itemOf(doc: Record<string, unknown>): OverlayThreadItem {
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 41, id: doc.id, doc }] }).items[0]
}

const secretDoc = (status: string) => ({
  id: 'sc_1',
  kind: 'secret',
  from: 'billing',
  created_at: 1761062000,
  secret: { name: 'STRIPE_KEY', status, prompt: 'Paste the live key', hosts: ['https://api.stripe.com:443'], delivery: 'fetch' },
})

describe('the sheet broker', () => {
  const pending = itemOf(secretDoc('pending'))
  const answered = itemOf({ ...secretDoc('set'), id: 'sc_2' })
  const plain = itemOf({ id: 'c1', kind: 'choice', from: 'b', choice: { prompt: 'Ok?', options: [], allow_custom: false, status: 'pending' } })

  it('opens only for a pending secret card, only after a press, one at a time', async () => {
    let pressed = false
    let close: () => void = () => undefined
    const shown: ZenSecretAsk[] = []
    const byId = new Map([pending, answered, plain].map((it) => [it.id, it]))
    const sheet = createZenSecretSheet({
      find: (_a, id) => byId.get(id) ?? null,
      show: (ask) =>
        new Promise<void>((r) => {
          shown.push(ask)
          close = r
        }),
      pressed: () => pressed,
    })
    expect([sheet.handles('billing', 'sc_1'), sheet.handles('billing', 'c1')]).toEqual([true, false])
    expect(await sheet.open('billing', 'c1')).toMatchObject({ ok: false, code: 'failed' })
    expect(await sheet.open('billing', 'sc_1')).toMatchObject({ ok: false, code: 'failed', message: expect.stringMatching(/clicks in the widget/) })
    pressed = true
    expect(await sheet.open('billing', 'sc_2')).toMatchObject({ ok: false, code: 'failed', message: expect.stringMatching(/isn’t waiting/) })
    const first = sheet.open('billing', 'sc_1')
    expect(await sheet.open('billing', 'sc_1')).toMatchObject({ ok: false, code: 'rate_limited' })
    close()
    expect(await first).toEqual({ ok: true })
    expect(shown).toEqual([{ address: 'billing', cardId: 'sc_1' }])
  })
})

describe('K2’s sheet outside the frame', () => {
  const scope = { label: 'This computer', serverSupports: () => false } as unknown as ServerScope

  it('draws the compact SecretCard and saves through the window’s path with no widget origin', async () => {
    const item = itemOf(secretDoc('pending'))
    const sent: Array<ThreadAnswerPayload & { secret: string }> = []
    const onDone = vi.fn()
    render(
      <ZenSecretSheet
        ask={{ address: 'billing::local', cardId: 'sc_1' }}
        answer={async (p) => {
          sent.push({ ...p })
        }}
        dismiss={async () => undefined}
        onDone={onDone}
        target={() => ({ item, scope, threadAddr: 'billing' })}
        onChange={() => () => undefined}
      />,
    )
    expect(screen.getByTestId('zen-secret-sheet').getAttribute('role')).toBe('dialog')
    expect(screen.getByTestId('thread-secret-card')).toBeTruthy()
    fireEvent.change(screen.getByTestId('thread-secret-field'), { target: { value: VALUE } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1))
    expect(sent).toHaveLength(1)
    expect(sent[0].secret).toBe(VALUE)
    // The person's own answer: never a widget origin on it.
    expect('origin' in sent[0]).toBe(false)
  })

  it('Close answers nothing; a card that’s gone says so', () => {
    const answer = vi.fn()
    const onDone = vi.fn()
    render(
      <ZenSecretSheet
        ask={{ address: 'a', cardId: 'x' }}
        answer={answer}
        dismiss={async () => undefined}
        onDone={onDone}
        target={() => null}
        onChange={() => () => undefined}
      />,
    )
    expect(screen.getByTestId('zen-secret-sheet-gone')).toBeTruthy()
    fireEvent.click(screen.getByTestId('zen-secret-sheet-close'))
    expect(onDone).toHaveBeenCalledTimes(1)
    expect(answer).not.toHaveBeenCalled()
  })
})

describe('the bridge and the broker together: the value never reaches the widget', () => {
  function payload(): ZenCustomWidgetPayload {
    return {
      id: 'thread',
      kind: 'custom',
      widget: 'garden-chat',
      column: 0,
      props: { config: {} },
      caps: ['agents:read', 'thread:read', 'thread:post'],
      requested: ['agents:read', 'thread:read', 'thread:post'],
      source: 'user',
      name: 'Garden chat',
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

  it('the widget’s call opens the sheet; the person’s Save goes out without the frame; the widget hears null', async () => {
    const item = itemOf(secretDoc('pending'))
    const innerCalls: string[] = []
    const inner = {
      widgetId: 'thread',
      caps: new Set<string>(),
      call: (verb: string) => {
        innerCalls.push(verb)
        return null
      },
    } as unknown as ZenWidgetBridge
    const personAnswers: unknown[] = []
    const secrets = createZenSecretSheet({
      find: (_a, id) => (id === item.id ? item : null),
      // What ZenCustomFrame does: K2's sheet, answered by the person.
      show: async (ask) => {
        personAnswers.push({ ask, secret: VALUE })
      },
      pressed: () => true,
    })
    const row = { address: 'billing::local', label: 'billing', index: 0, hostKey: 'local', openable: true } as never
    const deps: ZenCustomLayerDeps = {
      widget: payload,
      gardenId: () => 'g-test0001',
      inner,
      boundRows: () => [row],
      ensureConversation: async () => undefined,
      cardKind: (_a, id) => (id === item.id ? 'secret' : null),
      secrets,
      push: () => undefined,
      stop: () => undefined,
      paused: () => false,
      pauseForRunaway: () => undefined,
      theme: () => ({}),
      onThemeChange: () => () => undefined,
      now: () => 1_000_000,
    }
    const layer = createZenCustomLayer(deps)
    const reply = await layer.handle({ id: 1, verb: 'thread.answer', args: ['billing::local', 'sc_1'] })
    expect(reply).toEqual({ id: 1, ok: true, value: null })
    expect(JSON.stringify(reply)).not.toContain(VALUE)
    expect(personAnswers).toEqual([{ ask: { address: 'billing::local', cardId: 'sc_1' }, secret: VALUE }])
    // Nothing went through the widget's own answer path.
    expect(innerCalls).toEqual([])
  })
})
