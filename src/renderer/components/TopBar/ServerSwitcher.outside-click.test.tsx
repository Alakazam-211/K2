// @vitest-environment jsdom
//
// Two mounted switchers share one `open` flag. A display:none copy must
// not close the menu on mousedown, or the visible row is gone before click.
// Fail loud: a hidden listener that wins is the stuck-switcher bug.

import { afterEach, beforeEach, describe, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useServerSwitcherStore } from '@/stores/server-switcher'

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) =>
    sel({ openSettings: () => {} }),
}))

vi.mock('@/lib/remote-session', () => ({
  reviveRemoteSession: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

import ServerSwitcher from './ServerSwitcher'

const box: ConnectHost = {
  id: 'box',
  label: 'Box',
  hostname: 'box.k2.dev',
  port: 443,
  secure: true,
  token: 'tok',
  remember: false,
  lastConnectedAt: null,
}

const pickHost = vi.fn<(host: 'local' | ConnectHost) => void>()
const realPickHost = useConnectHostStore.getState().pickHost

function shell(container: HTMLElement, id: string): HTMLElement {
  const el = container.querySelector(`[data-testid="${id}"]`)
  if (!(el instanceof HTMLElement)) throw new Error(`missing ${id}`)
  return el
}

function optionRow(root: ParentNode, label: string): HTMLButtonElement {
  const found = Array.from(root.querySelectorAll('button[role="option"]')).find((b) =>
    b.textContent?.includes(label),
  )
  if (!(found instanceof HTMLButtonElement)) throw new Error(`no switcher row ${label}`)
  return found
}

function searchBox(root: ParentNode): HTMLInputElement {
  const el = root.querySelector('input[aria-label="Search servers"]')
  if (!(el instanceof HTMLInputElement)) throw new Error('search box missing')
  return el
}

function assertPickedBox(): void {
  const calls = pickHost.mock.calls
  if (calls.length !== 1) throw new Error(`expected pickHost once, saw ${calls.length}`)
  const got = calls[0][0]
  if (got === 'local' || got.id !== box.id) {
    throw new Error(`pickHost received ${got === 'local' ? 'local' : got.id}, expected ${box.id}`)
  }
}

function Pair(): React.JSX.Element {
  return (
    <>
      <div data-testid="hidden-switcher" style={{ display: 'none' }}>
        <ServerSwitcher />
      </div>
      <div data-testid="visible-switcher">
        <ServerSwitcher />
      </div>
    </>
  )
}

describe('server switcher outside click', () => {
  beforeEach(() => {
    // jsdom has no layout. The open menu scrolls the highlighted row into view.
    Element.prototype.scrollIntoView = () => {}
    pickHost.mockClear()
    act(() => {
      useServerSwitcherStore.setState({ open: false })
      useConnectHostStore.setState({
        activeHost: 'local',
        hosts: [box],
        pickHost,
      })
    })
  })

  afterEach(() => {
    cleanup()
    act(() => {
      useServerSwitcherStore.setState({ open: false })
    })
    useConnectHostStore.setState({
      pickHost: realPickHost,
      activeHost: 'local',
      hosts: [],
    })
    vi.restoreAllMocks()
  })

  it('keeps the visible row mounted through mousedown when a hidden switcher is listening', () => {
    const add = vi.spyOn(document, 'addEventListener')
    const { container } = render(<Pair />)
    const hidden = shell(container, 'hidden-switcher')
    const visible = shell(container, 'visible-switcher')
    if (getComputedStyle(hidden).display !== 'none') {
      throw new Error('hidden switcher is not display:none')
    }
    add.mockClear()
    act(() => {
      useServerSwitcherStore.getState().setOpen(true)
    })
    const mouseAdds = add.mock.calls.filter((c) => c[0] === 'mousedown')
    if (mouseAdds.length !== 1) {
      throw new Error(`expected one document mousedown listener, registered ${mouseAdds.length}`)
    }
    if (hidden.querySelector('[data-server-switcher]') == null) {
      throw new Error('hidden switcher did not mount a root')
    }
    if (visible.querySelector('[data-server-switcher]') == null) {
      throw new Error('visible switcher did not mount a root')
    }

    const row = optionRow(visible, 'Box')
    let openAtMouseDown: boolean | null = null
    const watch = (): void => {
      openAtMouseDown = useServerSwitcherStore.getState().open
    }
    document.addEventListener('mousedown', watch)
    fireEvent.mouseDown(row)
    document.removeEventListener('mousedown', watch)
    if (openAtMouseDown !== true) {
      throw new Error(`mousedown on the visible row set open to ${String(openAtMouseDown)} before click`)
    }
    if (!row.isConnected) throw new Error('visible row unmounted on mousedown, before click')
    if (pickHost.mock.calls.length !== 0) throw new Error('pickHost ran on mousedown')

    fireEvent.click(row)
    assertPickedBox()
  })

  it('closes on a mousedown outside both roots and does not pick', () => {
    const { container } = render(<Pair />)
    act(() => {
      useServerSwitcherStore.getState().setOpen(true)
    })
    if (!useServerSwitcherStore.getState().open) throw new Error('menu did not open')
    const outside = document.createElement('div')
    document.body.appendChild(outside)
    try {
      fireEvent.mouseDown(outside)
    } finally {
      outside.remove()
    }
    if (useServerSwitcherStore.getState().open) {
      throw new Error('mousedown outside both switchers left the menu open')
    }
    if (pickHost.mock.calls.length !== 0) {
      throw new Error('outside mousedown called pickHost')
    }
    if (container.querySelector('[role="listbox"]') != null) {
      throw new Error('menu stayed mounted after an outside mousedown')
    }
  })

  it('Enter on the highlighted search row calls pickHost with one switcher', () => {
    const { container } = render(<ServerSwitcher />)
    act(() => {
      useServerSwitcherStore.getState().setOpen(true)
    })
    const input = searchBox(container)
    fireEvent.change(input, { target: { value: 'box' } })
    const row = optionRow(container, 'Box')
    if (row.getAttribute('data-switcher-highlighted') !== 'true') {
      throw new Error('search did not highlight the Box row')
    }
    fireEvent.keyDown(input, { key: 'Enter' })
    assertPickedBox()
  })

  it('Enter on the highlighted search row calls pickHost with two switchers', () => {
    const { container } = render(<Pair />)
    act(() => {
      useServerSwitcherStore.getState().setOpen(true)
    })
    const visible = shell(container, 'visible-switcher')
    const input = searchBox(visible)
    fireEvent.change(input, { target: { value: 'box' } })
    const row = optionRow(visible, 'Box')
    if (row.getAttribute('data-switcher-highlighted') !== 'true') {
      throw new Error('search did not highlight the visible Box row')
    }
    fireEvent.keyDown(input, { key: 'Enter' })
    assertPickedBox()
  })
})
