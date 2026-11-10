// @vitest-environment jsdom
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, screen } from '@testing-library/react'
import { ThreadOverlayColumn, cssTimeMs } from './ThreadOverlayColumn'
import {
  EMPTY_START_GAP_PX,
  EMPTY_START_PAD_PX,
  emptyStartPlace,
} from './ThreadEmptyStart'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'

const room = testRoom({ tabs: {}, scope: primaryScope() })

const threadHook = vi.hoisted(() => ({
  items: [] as unknown[],
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loaded: true,
  outbox: [] as unknown[],
  loadOlder: async () => {},
}))
/** The hook's state changes re-render the (memoized) list, as in the app. */
const threadBus = vi.hoisted(() => {
  let version = 0
  const listeners = new Set<() => void>()
  return {
    get: () => version,
    sub: (fn: () => void) => {
      listeners.add(fn)
      return () => listeners.delete(fn)
    },
    bump: () => {
      version += 1
      for (const fn of listeners) fn()
    },
  }
})
vi.mock('./useOverlayThread', async () => {
  const { useSyncExternalStore } = await import('react')
  return {
    useOverlayThread: () => {
      useSyncExternalStore(threadBus.sub, threadBus.get)
      return threadHook
    },
  }
})
/** Change what the Thread holds, as the hook would. */
function setThread(next: Partial<typeof threadHook>): void {
  act(() => {
    Object.assign(threadHook, next)
    threadBus.bump()
  })
}
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))
const motion = vi.hoisted(() => ({ reduced: false }))
vi.mock('@/hooks/useReducedMotion', () => ({ useReducedMotion: () => motion.reduced }))
// The picture reads the projects and avatar stores: not under test here.
vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({
  AgentAvatar: () => null,
  ThreadSenderPicture: () => null,
}))

/** A laid-out column: 900 tall, a 60px message box, a 100px picture and name. */
const LAYOUT = { column: { w: 1000, h: 900 }, box: 60, start: 100 }
function stubLayout(): () => void {
  const proto = HTMLElement.prototype
  const saved = {
    clientHeight: Object.getOwnPropertyDescriptor(proto, 'clientHeight'),
    clientWidth: Object.getOwnPropertyDescriptor(proto, 'clientWidth'),
    offsetHeight: Object.getOwnPropertyDescriptor(proto, 'offsetHeight'),
    rect: proto.getBoundingClientRect,
  }
  const id = (el: HTMLElement): string | null => el.getAttribute('data-testid')
  Object.defineProperty(proto, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return id(this) === 'agent-session-thread' ? LAYOUT.column.h : 0
    },
  })
  Object.defineProperty(proto, 'clientWidth', {
    configurable: true,
    get(this: HTMLElement) {
      return id(this) === 'agent-session-thread' ? LAYOUT.column.w : 0
    },
  })
  Object.defineProperty(proto, 'offsetHeight', {
    configurable: true,
    get(this: HTMLElement) {
      if (id(this) === 'agent-session-thread-compose-box') return LAYOUT.box
      if (id(this) === 'thread-empty-start') return LAYOUT.start
      return 0
    },
  })
  proto.getBoundingClientRect = function (this: HTMLElement) {
    if (id(this) === 'agent-session-thread-compose-slot') return new DOMRect(0, 840, 1000, LAYOUT.box)
    return saved.rect.call(this)
  }
  return () => {
    for (const k of ['clientHeight', 'clientWidth', 'offsetHeight'] as const) {
      const d = saved[k]
      if (d) Object.defineProperty(proto, k, d)
      else delete (proto as unknown as Record<string, unknown>)[k]
    }
    proto.getBoundingClientRect = saved.rect
  }
}

const agentMessage = {
  collection: 'thread',
  seq: 1,
  id: 'm1',
  conversation_id: 'c',
  doc: { id: 'm1', kind: 'text', from: 'sales', to: null, created_at: 1700000000, body: 'Hello', via: null },
}

function renderColumn(): void {
  renderInRoom(
    room,
    <ThreadOverlayColumn
      addr="sales"
      conversationId="c"
      active
      centreEmpty
      agentName="Sales"
      composeBar={<textarea data-testid="field" />}
    />,
  )
}
const column = (): HTMLElement => screen.getByTestId('agent-session-thread')
const slot = (): HTMLElement => screen.getByTestId('agent-session-thread-compose-slot')

describe('emptyStartPlace', () => {
  it('centres the picture, the name and the box as one group', () => {
    const p = emptyStartPlace({ width: 1000, height: 900, inset: 60, box: 60, hero: 100 })!
    expect(p.fits).toBe(true)
    const total = 100 + EMPTY_START_GAP_PX + 60
    expect(p.top).toBe(Math.round((900 - total) * 0.45))
    // The box's top sits right under the name.
    expect(900 - 60 - p.lift).toBe(p.top + 100 + EMPTY_START_GAP_PX)
    expect(p.small).toBe(false)
  })

  it('top-aligns under the floats when short of room, and gives up when it cannot fit', () => {
    const short = emptyStartPlace({ width: 1000, height: 280, inset: 70, box: 60, hero: 100 })!
    expect(short.fits).toBe(true)
    expect(short.top).toBe(70 + EMPTY_START_PAD_PX)
    expect(short.small).toBe(true)
    const tiny = emptyStartPlace({ width: 1000, height: 200, inset: 70, box: 60, hero: 100 })!
    expect(tiny.fits).toBe(false)
    expect(tiny.lift).toBe(0)
  })

  it('waits for a laid-out column', () => {
    expect(emptyStartPlace({ width: 0, height: 0, inset: 0, box: 60, hero: 100 })).toBeNull()
    expect(emptyStartPlace({ width: 600, height: 900, inset: 0, box: 0, hero: 100 })).toBeNull()
  })

  it('a small column gets the smaller picture', () => {
    expect(emptyStartPlace({ width: 380, height: 900, inset: 0, box: 60, hero: 100 })!.small).toBe(true)
  })
})

describe('cssTimeMs', () => {
  it('reads the Style motion tokens', () => {
    expect(cssTimeMs(' 320ms')).toBe(320)
    expect(cssTimeMs('0.3s')).toBe(300)
    expect(cssTimeMs('')).toBeNull()
  })
})

describe('ThreadOverlayColumn: the empty start', () => {
  let restore: () => void
  beforeEach(() => {
    restore = stubLayout()
    motion.reduced = false
    threadHook.items = []
    threadHook.outbox = []
    threadHook.loaded = true
    threadHook.error = null
  })
  afterEach(() => {
    cleanup()
    restore()
    vi.useRealTimers()
  })

  it('an empty Thread starts centred: picture and name over the lifted box', () => {
    renderColumn()
    expect(column().hasAttribute('data-thread-centred')).toBe(true)
    const start = screen.getByTestId('thread-empty-start')
    expect(start.hasAttribute('data-shown')).toBe(true)
    expect(screen.getByTestId('thread-empty-start-name').textContent).toBe('Sales')
    expect(screen.getByTestId('thread-empty-start-picture').style.width).toBe('72px')
    const place = emptyStartPlace({ width: 1000, height: 900, inset: 0, box: 60, hero: 100 })!
    expect(start.style.top).toBe(`${place.top}px`)
    expect(slot().style.transform).toBe(`translateY(${-place.lift}px)`)
    // The plain empty line steps aside for it.
    expect(screen.queryByTestId('thread-overlay-empty')).toBeNull()
  })

  it('while the history loads it stays as it was: no centring, no flash', () => {
    threadHook.loaded = false
    renderColumn()
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
    expect(screen.queryByTestId('thread-empty-start')).toBeNull()
    expect(slot().style.transform).toBe('')
  })

  it('an error is not empty', () => {
    threadHook.error = 'offline'
    renderColumn()
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
  })

  it('a Thread with messages opens in the normal layout', () => {
    threadHook.items = [agentMessage]
    renderColumn()
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
    expect(column().hasAttribute('data-thread-drift')).toBe(false)
    expect(screen.queryByTestId('thread-empty-start')).toBeNull()
  })

  it('the first message drifts the box down in place (same field, focus kept)', () => {
    vi.useFakeTimers()
    renderInRoom(
      room,
      <ThreadOverlayColumn addr="sales" conversationId="c" active centreEmpty agentName="Sales" composeBar={<textarea data-testid="field" />} />,
    )
    const field = screen.getByTestId('field') as HTMLTextAreaElement
    field.focus()
    field.value = 'hi there'
    field.setSelectionRange(3, 3)
    expect(column().hasAttribute('data-thread-centred')).toBe(true)

    // A send shows at once as a pending bubble: that is the first message.
    setThread({ outbox: [{ ...agentMessage, id: 'p1', doc: { ...agentMessage.doc, id: 'p1', from: '' }, pending: { state: 'sending', clientMsgId: 'p1' } }] })
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
    expect(column().hasAttribute('data-thread-drift')).toBe(true)
    // The lift is gone in the same commit the drift's transition arrives in.
    expect(slot().style.transform).toBe('')
    // Fading out, not gone yet.
    expect(screen.getByTestId('thread-empty-start').hasAttribute('data-shown')).toBe(false)
    expect(screen.getByTestId('field')).toBe(field)
    expect(document.activeElement).toBe(field)
    expect(field.selectionStart).toBe(3)

    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(column().hasAttribute('data-thread-drift')).toBe(false)
    expect(screen.queryByTestId('thread-empty-start')).toBeNull()
    expect(document.activeElement).toBe(field)
  })

  it('reduced motion: an instant switch, no drift', () => {
    motion.reduced = true
    renderInRoom(
      room,
      <ThreadOverlayColumn addr="sales" conversationId="c" active centreEmpty agentName="Sales" composeBar={<textarea data-testid="field" />} />,
    )
    expect(column().hasAttribute('data-thread-centred')).toBe(true)
    // The agent's message arriving first drifts it too (here: at once).
    setThread({ items: [agentMessage] })
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
    expect(column().hasAttribute('data-thread-drift')).toBe(false)
    expect(screen.queryByTestId('thread-empty-start')).toBeNull()
    expect(slot().style.transform).toBe('')
  })

  it('a Thread emptied again starts centred again', () => {
    threadHook.items = [agentMessage]
    renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active centreEmpty agentName="Sales" composeBar={<textarea />} />)
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
    setThread({ items: [] })
    expect(column().hasAttribute('data-thread-centred')).toBe(true)
  })

  it('too short a column keeps the box at the bottom and the plain empty line', () => {
    const h = LAYOUT.column.h
    LAYOUT.column.h = 150
    try {
      renderColumn()
      expect(column().hasAttribute('data-thread-centred')).toBe(false)
      expect(slot().style.transform).toBe('')
      expect(screen.getByTestId('thread-overlay-empty')).toBeTruthy()
    } finally {
      LAYOUT.column.h = h
    }
  })

  it('off without centreEmpty, or without a message box', () => {
    renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={<textarea />} />)
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
    expect(screen.getByTestId('thread-overlay-empty')).toBeTruthy()
    cleanup()
    renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active centreEmpty composeBar={null} />)
    expect(column().hasAttribute('data-thread-centred')).toBe(false)
  })
})

describe("the header's picture and name while the Thread is empty", () => {
  const css = readFileSync(join(__dirname, '../../globals.css'), 'utf8')
  it('hide only the identity, only while a centred Thread is under the same floats', () => {
    const rule = /\[data-thread-floats-host\]:has\(\[data-thread-centred\]\) \[data-thread-floats\] \[data-thread-view-identity\] \{([^}]*)\}/.exec(css)
    expect(rule).not.toBeNull()
    expect(rule![1]).toMatch(/opacity: 0/)
    expect(rule![1]).toMatch(/visibility: hidden/)
    // No other float is hidden by the empty start.
    expect(css.match(/:has\(\[data-thread-centred\]\)[^{]*\{/g)).toHaveLength(1)
  })

  it('the drift uses the Style motion tokens and reduced motion turns it off', () => {
    expect(css).toMatch(/\[data-thread-drift\] > \[data-thread-compose-slot\] \{ transition: transform var\(--motion-duration-slow\) var\(--motion-ease-out-soft\); \}/)
    const reduce = css.slice(css.indexOf('[data-thread-drift] > [data-thread-compose-slot] { transition'))
    expect(reduce).toMatch(/@media \(prefers-reduced-motion: reduce\) \{\s*\[data-thread-view-identity\],/)
  })
})
