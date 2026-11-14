// @vitest-environment jsdom
// Thread view's header (prd-thread-view-focus-v1 §4): the picture above the
// name, as on the phone; one line in short panes; the server under the name
// in a room on another server. No status dot (dropped from v1: the phone's
// header has none). The name keeps its size: its object grows and gives way
// around it, in the phone's order (identityFit).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, screen } from '@testing-library/react'

const avatar = vi.hoisted(() => ({
  calls: [] as Array<{ addr: string; label: string; host: string | null; size?: number }>,
}))
vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({
  AgentAvatar: (p: { addr: string; label: string; host: string | null; size?: number }) => {
    avatar.calls.push(p)
    return <span data-testid="avatar">{p.label}</span>
  },
}))

import { IDENTITY_NAME_MIN_PX, IDENTITY_PAD_PX, ThreadViewIdentity, identityCompact, identityFit } from './ThreadViewIdentity'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { primaryScope } from '@/kessel/server-scope'

const primary = testRoom({ tabs: {}, scope: primaryScope() })
const remote = testRoom({
  tabs: {},
  scope: { ...fakeScope('box.k2.dev'), label: 'Office box' },
})

let paneSize = { width: 800, height: 700 }
let observed: Array<() => void> = []

function framed(name: string, addr: string): React.JSX.Element {
  return (
    <div data-testid="pane">
      <div data-session-header="">
        <ThreadViewIdentity name={name} addr={addr} />
      </div>
    </div>
  )
}

beforeEach(() => {
  avatar.calls = []
  observed = []
  paneSize = { width: 800, height: 700 }
  vi.stubGlobal(
    'ResizeObserver',
    class {
      cb: () => void
      constructor(cb: () => void) {
        this.cb = cb
      }
      observe() {
        observed.push(this.cb)
      }
      disconnect() {}
      unobserve() {}
    },
  )
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return this.dataset.testid === 'pane'
      ? new DOMRect(0, 0, paneSize.width, paneSize.height)
      : new DOMRect(0, 0, 10, 10)
  })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('identityCompact', () => {
  it('one line under 420 tall, not at the edge, not at size 0', () => {
    expect(identityCompact(419)).toBe(true)
    expect(identityCompact(420)).toBe(false)
    expect(identityCompact(0)).toBe(false)
  })
})

describe('identityFit: the name keeps its size, the object gives way around it', () => {
  const pad = IDENTITY_PAD_PX
  it('with room, the object is the text and its roomy padding', () => {
    expect(identityFit({ name: 200, server: 90, room: 600 })).toEqual({
      pad: 'roomy',
      width: 200 + 2 * pad.roomy,
      serverClips: false,
      nameClips: false,
    })
  })

  it('first the padding tightens', () => {
    const room = 200 + 2 * pad.roomy - 1
    expect(identityFit({ name: 200, server: 90, room })).toEqual({
      pad: 'snug',
      width: 200 + 2 * pad.snug,
      serverClips: false,
      nameClips: false,
    })
  })

  it('then the server line shortens, while the name still fits whole', () => {
    // The server's line is the longer: it gives way, the name doesn't.
    const f = identityFit({ name: 150, server: 240, room: 200 })
    expect(f).toEqual({ pad: 'snug', width: 200, serverClips: true, nameClips: false })
  })

  it('the name shortens last, and never below its minimum', () => {
    expect(identityFit({ name: 300, server: 90, room: 200 })).toMatchObject({ width: 200, nameClips: true, serverClips: false })
    const tiny = identityFit({ name: 300, room: 40 })
    expect(tiny.width).toBe(IDENTITY_NAME_MIN_PX + 2 * pad.snug)
    // A short name is never padded out to the minimum.
    expect(identityFit({ name: 30, room: 40 }).width).toBe(30 + 2 * pad.snug)
  })

  it('the text itself never shrinks: the fit only ever changes the object', () => {
    for (const room of [600, 300, 200, 120]) {
      const f = identityFit({ name: 250, server: 120, room })
      expect(Object.keys(f).sort()).toEqual(['nameClips', 'pad', 'serverClips', 'width'])
    }
  })

  it('in one line, the picture sits inside and the server is on hover', () => {
    const f = identityFit({ name: 100, server: 300, room: 160, compact: true })
    expect(f).toMatchObject({ pad: 'roomy', serverClips: false, nameClips: false })
    expect(identityFit({ name: 100, room: 135, compact: true }).pad).toBe('snug')
  })
})

describe('ThreadViewIdentity', () => {
  it('stacks a 32px picture above the name', async () => {
    renderInRoom(primary, framed('Sales', 'sales/reviewer'))
    await screen.findByTestId('avatar')
    const root = screen.getByTestId('thread-view-identity')
    expect(root.className).toContain('flex-col')
    expect(root.firstElementChild).toBe(screen.getByTestId('thread-view-picture'))
    expect(screen.getByTestId('thread-view-name').textContent).toBe('Sales')
    expect(screen.getByTestId('thread-view-picture').style.width).toBe('36px')
    expect(avatar.calls.at(-1)).toMatchObject({ addr: 'sales/reviewer', label: 'Sales', host: null, size: 36 })
  })

  it('paints only from theme tokens, and the picture takes the pill radius', () => {
    renderInRoom(primary, framed('Sales', 'sales'))
    expect(screen.getByTestId('thread-view-picture').style.borderRadius).toBe('var(--radius-pill)')
    expect(screen.getByTestId('thread-view-name').style.fontSize).toBe('var(--type-lg)')
  })

  it('names the server only in a room on another server', async () => {
    renderInRoom(primary, framed('Sales', 'sales'))
    expect(screen.queryByTestId('thread-view-server')).toBeNull()
    cleanup()
    renderInRoom(remote, framed('Sales', 'sales'))
    expect(screen.getByTestId('thread-view-server').textContent).toBe('Office box')
    await screen.findByTestId('avatar')
    expect(avatar.calls.at(-1)?.host).toBe('box.k2.dev')
  })

  it('has no status dot', () => {
    renderInRoom(primary, framed('Sales', 'sales'))
    expect(document.querySelector('[data-testid="thread-view-status"]')).toBeNull()
    expect(screen.getByTestId('thread-view-identity').children).toHaveLength(2)
  })

  it('drops to one line in a short pane, and returns when it grows', () => {
    paneSize = { width: 800, height: 400 }
    renderInRoom(primary, framed('Sales', 'sales'))
    const root = screen.getByTestId('thread-view-identity')
    expect(root.hasAttribute('data-compact')).toBe(true)
    expect(root.className).toContain('flex-row')
    expect(screen.getByTestId('thread-view-picture').style.width).toBe('20px')

    paneSize = { width: 800, height: 700 }
    act(() => {
      for (const fire of observed) fire()
    })
    expect(screen.getByTestId('thread-view-identity').hasAttribute('data-compact')).toBe(false)
    expect(screen.getByTestId('thread-view-picture').style.width).toBe('36px')
  })

  it('in a short pane, keeps the server on hover rather than a second line', () => {
    paneSize = { width: 800, height: 400 }
    renderInRoom(remote, framed('Sales', 'sales'))
    expect(screen.queryByTestId('thread-view-server')).toBeNull()
    expect(screen.getByTestId('thread-view-identity').title).toBe('Office box')
  })

  it('a narrow pane keeps the picture above the name: the name has the whole row under it', () => {
    paneSize = { width: 300, height: 700 }
    renderInRoom(remote, framed('Sales', 'sales'))
    expect(screen.getByTestId('thread-view-identity').hasAttribute('data-compact')).toBe(false)
    expect(screen.getByTestId('thread-view-server').textContent).toBe('Office box')
  })

  it('follows a renamed agent', async () => {
    const { rerender } = renderInRoom(primary, framed('Sales', 'sales'))
    await screen.findByTestId('avatar')
    expect(screen.getByTestId('thread-view-name').textContent).toBe('Sales')
    rerender(framed('Sales Desk', 'sales'))
    expect(screen.getByTestId('thread-view-name').textContent).toBe('Sales Desk')
    expect(avatar.calls.at(-1)?.label).toBe('Sales Desk')
  })
})

describe('in the floats’ row', () => {
  // jsdom lays nothing out: the row's and the text's widths, in CSS px.
  let rowWidth = 800
  const textWidth: Record<string, number> = {}
  beforeEach(() => {
    rowWidth = 800
    textWidth['thread-view-name'] = 180
    textWidth['thread-view-server'] = 70
    const own = <T,>(key: keyof HTMLElement, get: (el: HTMLElement) => T | undefined) => {
      const base = Object.getOwnPropertyDescriptor(HTMLElement.prototype, key) ?? Object.getOwnPropertyDescriptor(Element.prototype, key)
      vi.spyOn(HTMLElement.prototype, key as 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
        const v = get(this)
        return (v ?? base?.get?.call(this) ?? 0) as number
      })
    }
    own('clientWidth', (el) => (el.dataset.testid === 'row' ? rowWidth + 24 : undefined))
    own('offsetWidth', (el) => (el.dataset.testid === 'row' ? rowWidth + 24 : undefined))
    own('scrollWidth', (el) => textWidth[el.dataset.testid ?? ''])
  })

  function inRow(name: string): React.JSX.Element {
    return (
      <div data-testid="pane">
        <div data-session-header="" data-thread-floats="">
          <div data-testid="row" data-thread-floats-grid="" style={{ paddingLeft: 12, paddingRight: 12 }}>
            <div className="thread-float-group" />
            <div>
              <ThreadViewIdentity name={name} addr="sales" />
            </div>
            <div className="thread-float-group" />
          </div>
        </div>
      </div>
    )
  }
  const root = () => screen.getByTestId('thread-view-identity')
  const refit = () =>
    act(() => {
      for (const fire of observed) fire()
    })

  it('the name’s object is as wide as its text, out past the picture’s column', () => {
    renderInRoom(remote, inRow('Sales'))
    expect(root().getAttribute('data-fit')).toBe('roomy')
    expect(root().hasAttribute('data-plate')).toBe(true)
    // The text (181, a px for rounding) and 14px each side.
    expect(root().style.getPropertyValue('--k2-plate-w')).toBe('209px')
    // Its margins give back all but the picture's 36px: its column stays the picture's.
    expect(root().style.getPropertyValue('--k2-plate-ml')).toBe('-86.5px')
    expect(root().style.getPropertyValue('--k2-plate-mr')).toBe('-86.5px')
    expect(screen.getByTestId('thread-view-name').style.fontSize).toBe('var(--type-lg)')
  })

  it('narrowing: the padding goes first, then the object stops at the row’s edges', () => {
    renderInRoom(remote, inRow('Sales'))
    rowWidth = 200
    refit()
    expect(root().getAttribute('data-fit')).toBe('snug')
    expect(root().style.getPropertyValue('--k2-plate-w')).toBe('197px')
    rowWidth = 150
    refit()
    expect(root().style.getPropertyValue('--k2-plate-w')).toBe('150px')
    // The name is whole on hover.
    expect(screen.getByTestId('thread-view-name').title).toBe('Sales')
    // The text's size never changed.
    expect(screen.getByTestId('thread-view-name').style.fontSize).toBe('var(--type-lg)')
  })

  it('in one line, sits between the objects (no reach), with its minimum', () => {
    paneSize = { width: 800, height: 400 }
    renderInRoom(primary, inRow('Sales'))
    expect(root().hasAttribute('data-compact')).toBe(true)
    expect(root().hasAttribute('data-plate')).toBe(false)
    expect(screen.getByTestId('thread-view-name').style.minWidth).toBe(`${IDENTITY_NAME_MIN_PX}px`)
  })
})
