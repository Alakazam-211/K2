// @vitest-environment jsdom
// Thread view's header (prd-thread-view-focus-v1 §4): the picture above the
// name, as on the phone; one line in small panes; the server under the name
// in a room on another server. No status dot (dropped from v1: the phone's
// header has none).
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

import { ThreadViewIdentity, identityCompact } from './ThreadViewIdentity'
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
  it('one line under 420 tall or 360 wide, not at the edge, not at size 0', () => {
    expect(identityCompact(800, 419)).toBe(true)
    expect(identityCompact(359, 700)).toBe(true)
    expect(identityCompact(800, 420)).toBe(false)
    expect(identityCompact(360, 700)).toBe(false)
    expect(identityCompact(0, 0)).toBe(false)
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
    expect(screen.getByTestId('thread-view-picture').style.width).toBe('32px')
    expect(avatar.calls.at(-1)).toMatchObject({ addr: 'sales/reviewer', label: 'Sales', host: null, size: 32 })
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
    expect(screen.getByTestId('thread-view-picture').style.width).toBe('32px')
  })

  it('in a narrow pane, keeps the server on hover rather than a second line', () => {
    paneSize = { width: 300, height: 700 }
    renderInRoom(remote, framed('Sales', 'sales'))
    expect(screen.queryByTestId('thread-view-server')).toBeNull()
    expect(screen.getByTestId('thread-view-identity').title).toBe('Office box')
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
