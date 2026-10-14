// Garden v3 (contract-sources-v1 §11.4, GV49) — what a widget frame's run
// reports carry: bridge refusals as fixed codes, and for a widget that reads
// data sources, never message or stack text.
import { describe, expect, it, vi } from 'vitest'
import type { ZenCustomLayer } from './zen-custom-bridge'
import type { ZenFrameHello, ZenFrameReply } from './zen-custom-types'
import { startZenFrameHost, type ZenFrameReport } from './zen-custom-host'
import { zenErrorClass, zenErrorLine, zenRunReportBody } from './zen-widget-routes'

const HELLO: ZenFrameHello = {
  k2: 'hello',
  v: 1,
  caps: ['agents:read'],
  features: [],
  widget: { id: 'fleet', name: 'Fleet', garden: 'g-test0001' },
  config: {},
  motion: { reduced: false },
}

function host(replies: (m: Record<string, unknown>) => ZenFrameReply | null): {
  deliver(m: unknown): void
  reports: ZenFrameReport[]
  sent: unknown[]
} {
  const sent: unknown[] = []
  let onmessage: ((e: MessageEvent) => void) | null = null
  const port = {
    postMessage: (m: unknown) => void sent.push(m),
    close: () => undefined,
    set onmessage(f: ((e: MessageEvent) => void) | null) {
      onmessage = f
    },
    get onmessage() {
      return onmessage
    },
  }
  const reports: ZenFrameReport[] = []
  startZenFrameHost(
    { postMessage: () => undefined },
    {
      hello: HELLO,
      label: 'fleet',
      channel: () => ({ port1: port, port2: {} }) as unknown as MessageChannel,
      stop: () => undefined,
      focused: () => false,
      onChord: () => undefined,
      onReport: (r) => void reports.push(r),
      now: () => Date.now(),
      makeLayer: (): ZenCustomLayer => ({
        handle: async (m) => replies(m as Record<string, unknown>),
        subscriptions: 0,
        dispose: () => undefined,
      }),
    },
  )
  return { deliver: (m) => onmessage?.({ data: m } as MessageEvent), reports, sent }
}

describe('bridge refusals reach the runtime log as codes', () => {
  it('reports not_bound, unreachable and the rest; never ordinary failures', async () => {
    vi.useFakeTimers()
    const h = host((m) => {
      const id = m.id as number
      const code = String((m.args as unknown[])[0])
      return code === 'ok' ? { id, ok: true, value: 1 } : { id, ok: false, error: { code: code as never, message: 'secret value 42' } }
    })
    h.deliver({ id: 1, verb: 'sources.read', args: ['not_bound'] })
    h.deliver({ id: 2, verb: 'sources.read', args: ['unreachable'] })
    h.deliver({ id: 3, verb: 'sources.read', args: ['failed'] })
    h.deliver({ id: 4, verb: 'sources.read', args: ['ok'] })
    h.deliver({ id: 5, verb: 'thread.post', args: ['cap_not_granted'] })
    await vi.runAllTimersAsync()
    expect(h.reports).toEqual([
      { event: 'refusal', verb: 'sources.read', code: 'not_bound' },
      { event: 'refusal', verb: 'sources.read', code: 'unreachable' },
      { event: 'refusal', verb: 'thread.post', code: 'cap_not_granted' },
    ])
    expect(JSON.stringify(h.reports)).not.toContain('42')
    vi.useRealTimers()
  })
})

describe('zenRunReportBody (GV49)', () => {
  const err = { event: 'error' as const, message: 'TypeError: 4242 is not a function', stack: 'at about:srcdoc:17:9\nat x:1:1' }

  it('a widget that reads sources sends class and line only', () => {
    const b = zenRunReportBody('fleet', 'h'.repeat(64), err, true)
    expect(b).toEqual({ widget: 'fleet', hash: 'h'.repeat(64), event: 'error', kind: 'error', class: 'TypeError', line: 17 })
    expect(JSON.stringify(b)).not.toContain('4242')
  })

  it('other widgets keep message and stack, plus the fixed fields', () => {
    const b = zenRunReportBody('arcade', 'h', err, false)
    expect(b).toMatchObject({ message: err.message, stack: err.stack, class: 'TypeError', line: 17, kind: 'error' })
  })

  it('refusals and ready', () => {
    expect(zenRunReportBody('w', 'h', { event: 'refusal', verb: 'sources.read', code: 'not_bound' }, true)).toEqual({
      widget: 'w',
      hash: 'h',
      event: 'error',
      kind: 'refusal',
      class: 'sources.read',
      code: 'not_bound',
    })
    expect(zenRunReportBody('w', 'h', { event: 'ready' }, true)).toEqual({ widget: 'w', hash: 'h', event: 'ready' })
  })

  it('class and line parsing', () => {
    expect(zenErrorClass('ReferenceError: x')).toBe('ReferenceError')
    expect(zenErrorClass('value 12 broke')).toBeUndefined()
    expect(zenErrorClass('a b: c')).toBeUndefined()
    expect(zenErrorLine('at about:srcdoc:3:1')).toBe(3)
    expect(zenErrorLine(undefined)).toBeUndefined()
  })
})
