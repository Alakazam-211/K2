// @vitest-environment jsdom
// Settings → General, rebuilt as card groups like Workspaces / Agents: the
// update states, the reset confirm, the server row, and the Connected
// server card for a server too old to restart or update from here.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const tauri = vi.hoisted(() => ({
  calls: [] as string[],
  cli: { installed: true, installedVersion: '0.46.11', bundledVersion: '0.46.11', updateAvailable: false } as Record<string, unknown>,
  daemon: { state: 'running', version: '0.46.11', uptime_secs: 3600, pid: 42, port: 7777 } as Record<string, unknown>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    tauri.calls.push(cmd)
    if (cmd === 'get_current_version') return '0.46.11'
    if (cmd === 'cli_install_status') return tauri.cli
    if (cmd === 'daemon_status') return tauri.daemon
    if (cmd === 'daemon_restart') return null
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: vi.fn(async () => ({ owner: true, role: 'owner' })),
    daemonCliPost: vi.fn(async () => ({})),
  }
})
vi.mock('@/lib/daemon-settings', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-settings')>()
  return { ...actual, settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})) }
})
vi.mock('@/lib/git-tools', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/git-tools')>()
  return { ...actual, fetchGitTools: vi.fn(async () => Promise.reject(new Error('older server'))) }
})
vi.mock('./useNotifyPrefs', () => ({ threadPushAvailable: () => false }))

import { GeneralSection } from './GeneralSection'
import { useSettingsStore } from '@/stores/settings'
import { useUpdateStore } from '@/stores/update'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'

const confirm = vi.fn(async () => true)
const reset = vi.fn(async () => {})

beforeEach(() => {
  tauri.calls = []
  tauri.cli = { installed: true, installedVersion: '0.46.11', bundledVersion: '0.46.11', updateAvailable: false }
  tauri.daemon = { state: 'running', version: '0.46.11', uptime_secs: 3600, pid: 42, port: 7777 }
  confirm.mockReset().mockResolvedValue(true)
  reset.mockReset().mockResolvedValue(undefined)
  useConfirmDialogStore.setState({ confirm })
  useSettingsStore.setState({ resetAllSettings: reset, generalSubTab: null, pendingUpdateCheck: false })
  useUpdateStore.setState({ status: 'idle', version: null, error: null, progress: 0, archDownloadUrl: null, linuxManualCommand: null })
  useToastStore.setState({ toasts: [] })
  useConnectHostStore.setState({ activeHost: 'local' })
})

afterEach(() => cleanup())

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-settings-id="${id}"]`)
  if (!el) throw new Error(`no row ${id}`)
  return el
}

describe('Settings → General', () => {
  it('is card groups under one title and tab strip, with no bare section heading', async () => {
    render(<GeneralSection />)
    expect(screen.getByRole('heading', { name: 'General' })).toBeTruthy()
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['General', 'Server'])
    const groups = screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)
    // "You" always has Your timezone, so it is never a bare heading even
    // when Your account is hidden (a Member, or no owner-linked accounts).
    expect(groups).toEqual(['K2', 'Text', 'This computer', 'You', 'Saved settings'])
    expect(within(row('general.owner-timezone')).getByLabelText('Your timezone')).toBeTruthy()
    await waitFor(() => expect(row('general.app-version').textContent).toContain('Version 0.46.11.'))
    expect(row('general.cli-version').textContent).toContain('Version 0.46.11 installed.')
  })

  it('an update on offer is the one accent button; the k2 command Install steps back', async () => {
    tauri.cli = { installed: false, installedVersion: null, bundledVersion: '0.46.11', updateAvailable: false }
    useUpdateStore.setState({ status: 'available', version: '0.47.0' })
    render(<GeneralSection />)
    const download = within(row('general.app-version')).getByRole('button', { name: 'Download' })
    expect(download.className).toContain('accent-fill')
    const install = await within(row('general.cli-version')).findByRole('button', { name: 'Install' })
    expect(install.className).not.toContain('accent-fill')
    expect(within(row('general.app-version')).queryByRole('button', { name: 'Check for updates' })).toBeNull()
  })

  it('a failed update names the failure and offers Try again and the release page', () => {
    useUpdateStore.setState({ status: 'error', error: 'signature did not verify' })
    render(<GeneralSection />)
    const alert = within(row('general.app-version')).getByRole('alert')
    expect(alert.textContent).toContain("K2 couldn't finish the update")
    expect(alert.textContent).toContain('signature did not verify')
    expect(within(alert).getByRole('button', { name: 'Try again' })).toBeTruthy()
    expect(within(alert).getByRole('button', { name: 'Open release page' })).toBeTruthy()
  })

  it('a download shows its progress', () => {
    useUpdateStore.setState({ status: 'downloading', version: '0.47.0', progress: 40 })
    render(<GeneralSection />)
    const bar = within(row('general.app-version')).getByRole('progressbar')
    expect(bar.getAttribute('aria-valuenow')).toBe('40')
    expect(row('general.app-version').textContent).toContain('Downloading K2 0.47.0…')
  })

  it('Reset is quiet at rest, asks first (destructive, owner only), then says so on the row and in a toast', async () => {
    render(<GeneralSection />)
    const button = within(row('general.reset-all')).getByRole('button', { name: 'Reset…' })
    // Present but quiet: an outline with an error label, red only under the pointer.
    expect(button.className).toContain('text-[var(--color-status-error-text)]')
    expect(button.className).not.toMatch(/(^|\s)bg-\[var\(--color-danger/)
    await act(async () => {
      fireEvent.click(button)
    })
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: 'Reset all settings?', destructive: true }))
    expect((confirm.mock.calls[0] as unknown as [{ message: string }])[0].message).toContain("Only this server's owner can reset it.")
    expect(reset).toHaveBeenCalledTimes(1)
    expect(within(row('general.reset-all')).getByRole('status').textContent).toBe('Settings reset to defaults.')
    expect(useToastStore.getState().toasts.map((t) => t.message)).toContain('Settings are back to their defaults.')
  })

  it('Check for updates that finds nothing says so on the K2 app row', async () => {
    useUpdateStore.setState({ checkForUpdate: async () => false } as never)
    render(<GeneralSection />)
    await act(async () => {
      fireEvent.click(within(row('general.app-version')).getByRole('button', { name: 'Check for updates' }))
    })
    await waitFor(() => expect(screen.getByTestId('app-version-label').textContent).toContain('Up to date. Checked just now.'))
  })

  it('the tabs show keyboard focus in the theme\'s shape', () => {
    render(<GeneralSection />)
    for (const tab of screen.getAllByRole('tab')) {
      expect(tab.hasAttribute('data-k2-focus')).toBe(true)
      expect(tab.className).toContain('relative')
      expect(tab.className).not.toContain('focus-visible:outline-')
    }
  })

  it('a server switch says Saved when the write lands', async () => {
    render(<GeneralSection />)
    act(() => void fireEvent.click(screen.getByRole('tab', { name: 'Server' })))
    await act(async () => {
      fireEvent.click(screen.getByRole('switch', { name: 'Keep server running when the window is closed' }))
    })
    await waitFor(() => expect(row('general.keep-daemon-on-quit').querySelector('[data-save-note]')?.textContent).toBe('Saved'))
  })

  it('a server that isn\'t answering is said plainly, with no raw error in the sentence', async () => {
    tauri.daemon = { state: 'unreachable', reason: 'connection refused' }
    render(<GeneralSection />)
    act(() => void fireEvent.click(screen.getByRole('tab', { name: 'Server' })))
    await waitFor(() => expect(within(row('general.daemon')).getByRole('alert').textContent).toContain("isn't answering. Restart it"))
    expect(within(row('general.daemon')).getByRole('alert').textContent).not.toContain('connection refused')
  })

  it('Reset cancelled changes nothing; a refused reset says why on the row', async () => {
    confirm.mockResolvedValueOnce(false)
    render(<GeneralSection />)
    const button = within(row('general.reset-all')).getByRole('button', { name: 'Reset…' })
    await act(async () => {
      fireEvent.click(button)
    })
    expect(reset).not.toHaveBeenCalled()
    reset.mockRejectedValueOnce(new Error('settings_reset 403: resetting settings requires the Owner role'))
    await act(async () => {
      fireEvent.click(button)
    })
    expect(within(row('general.reset-all')).getByRole('alert').textContent).toBe(
      "Only this server's owner can reset its settings. Nothing was changed.",
    )
  })

  it('Server tab: the server row shows its state and Restart asks first', async () => {
    render(<GeneralSection />)
    act(() => void fireEvent.click(screen.getByRole('tab', { name: 'Server' })))
    await waitFor(() => expect(row('general.daemon').textContent).toContain('Running · version 0.46.11 · up 1h 0m'))
    await act(async () => {
      fireEvent.click(within(row('general.daemon')).getByRole('button', { name: 'Restart' }))
    })
    expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: 'Restart the K2 server?' }))
    await waitFor(() => expect(tauri.calls).toContain('daemon_restart'), { timeout: 3000 })
    await waitFor(
      () => expect(useToastStore.getState().toasts.map((t) => t.message)).toContain('The K2 server restarted.'),
      { timeout: 3000 },
    )
  })
})

describe('Settings → General, connected to another server', () => {
  const OLD: ConnectHost = {
    id: 'id-old',
    label: 'Lab Mac',
    hostname: 'lab.k2.dev',
    port: 443,
    secure: true,
    username: 'rosson',
    token: 'tok',
    remember: true,
    lastConnectedAt: null,
  }

  it('a server too old to restart or update says so in the Connected server card', () => {
    useConnectHostStore.setState({ activeHost: OLD, hosts: [OLD], serverVersion: '0.30.0', serverProtocol: 1 })
    render(<GeneralSection />)
    expect(screen.getByRole('heading', { name: 'Connected server' })).toBeTruthy()
    expect(screen.getByTestId('remote-host-unsupported').textContent).toContain('Lab Mac runs a K2 too old to restart or update from here')
    // The status is a tag, like other rows' states, not bare text.
    expect(within(screen.getByTestId('remote-host-unsupported')).getByText('Not available').className).toContain('k2-tag')
    // This computer's version is marked so it isn't taken for the server's.
    expect(within(row('general.app-version')).getByText('This computer')).toBeTruthy()
  })
})
