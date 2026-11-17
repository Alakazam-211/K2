// @vitest-environment jsdom
//
// The server menu's connection mark is the app's status shape: its corners
// read `--radius-status`, so it is a round dot in Glass and a square in
// Terminal, the same as every other status mark. Only the current server
// has a known connection, so only its row carries a mark; the other rows
// keep a blank slot of the same width so the names line up.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useServerSwitcherStore } from '@/stores/server-switcher'

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) => sel({ openSettings: () => {} }),
}))
vi.mock('@/lib/remote-session', () => ({ reviveRemoteSession: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import ServerSwitcher from './ServerSwitcher'

const repo = join(__dirname, '..', '..', '..', '..')
const generated = readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8')

function rule(sel: string): string {
  const at = generated.indexOf(`${sel} {`)
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(0)
  return generated.slice(at, generated.indexOf('}', at))
}

function host(id: string): ConnectHost {
  return { id, label: id, hostname: `${id}.k2.dev`, port: 443, secure: true, token: 'tok', remember: false, lastConnectedAt: null }
}

const realPickHost = useConnectHostStore.getState().pickHost
const realStatus = useConnectHostStore.getState().connectionStatus

function openOn(active: 'local' | ConnectHost, hosts: ConnectHost[]): HTMLElement {
  act(() => {
    useConnectHostStore.setState({ activeHost: active, hosts, pickHost: vi.fn(), connectionStatus: 'connected' })
  })
  const { container } = render(<ServerSwitcher />)
  act(() => {
    useServerSwitcherStore.getState().setOpen(true)
  })
  return container
}

function rows(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>('button[role="option"]'))
}

const STATUS_RADIUS = 'var(--radius-status, 0px)'

beforeEach(() => {
  Element.prototype.scrollIntoView = () => {}
})

afterEach(() => {
  cleanup()
  act(() => {
    useServerSwitcherStore.getState().setOpen(false)
    useConnectHostStore.setState({ activeHost: 'local', hosts: [], pickHost: realPickHost, connectionStatus: realStatus })
  })
})

describe('server menu status mark', () => {
  it('the status token is round in Glass and square in Terminal', () => {
    expect(rule('[data-style="glass"]')).toMatch(/--radius-status: 9999px;/)
    expect(rule('[data-style="terminal"]')).toMatch(/--radius-status: 0px;/)
  })

  it('a selected remote server: its mark reads the status radius; the others keep a blank slot', () => {
    const a = host('alpha')
    const b = host('beta')
    const root = openOn(a, [a, b])
    const all = rows(root)
    expect(all.length).toBe(3)
    for (const row of all) {
      const mark = row.querySelector<HTMLElement>('[data-server-status]')
      if (row.id === 'server-switcher-opt-alpha') {
        expect(mark, 'selected row has a mark').not.toBeNull()
        expect(mark?.getAttribute('data-server-status')).toBe('connected')
        expect(mark?.style.borderRadius).toBe(STATUS_RADIUS)
      } else {
        expect(mark, `${row.id} has no mark`).toBeNull()
        expect(row.querySelector('[data-server-status-slot]'), `${row.id} keeps the slot`).not.toBeNull()
      }
    }
  })

  it('This computer selected: its mark reads the status radius too', () => {
    const root = openOn('local', [host('alpha')])
    const mark = root.querySelector<HTMLElement>('#server-switcher-opt-local [data-server-status]')
    expect(mark).not.toBeNull()
    expect(mark?.style.borderRadius).toBe(STATUS_RADIUS)
    expect(root.querySelectorAll('[data-server-status]').length).toBe(1)
  })
})
