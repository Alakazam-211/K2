// @vitest-environment jsdom
// Settings → Accessibility: each macOS permission in one card row with its
// status (Allowed / Not allowed / Can't tell) and one action, the page that
// keeps asking macOS so a permission turned on in System Settings flips by
// itself, a failed read said once, and a failed open said on its row.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within, act } from '@testing-library/react'

const invoke = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke }))

import { PermissionsSection, PERMISSIONS_MANIFEST } from './PermissionsSection'

type Status = { fullDiskAccess: boolean; accessibility: boolean; microphone: boolean }
let status: Status | Error
let requestReply: unknown
let requestError: string | null

function setUp(): void {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'permissions_get_status') {
      if (status instanceof Error) throw status.message
      return status
    }
    if (requestError) throw requestError
    return requestReply
  })
}

const row = (id: string): HTMLElement => {
  const el = document.querySelector<HTMLElement>(`[data-settings-id="permissions-${id}"]`)
  if (!el) throw new Error(`no row ${id}`)
  return el
}
const statusOf = (id: string): string | null =>
  row(id).querySelector('[data-permission-status]')?.getAttribute('data-permission-status') ?? null

beforeEach(() => {
  status = { fullDiskAccess: false, accessibility: true, microphone: true }
  requestReply = null
  requestError = null
  invoke.mockReset()
  setUp()
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('PermissionsSection', () => {
  it('shows each permission with its status and an action only where one is needed', async () => {
    render(<PermissionsSection />)
    await waitFor(() => expect(statusOf('microphone')).toBe('allowed'))
    expect(statusOf('full-disk-access')).toBe('not-allowed')
    expect(statusOf('accessibility')).toBe('allowed')
    expect(statusOf('apple-events')).toBe('unknown')
    expect(statusOf('local-network')).toBe('unknown')
    expect(within(row('full-disk-access')).getByText('Not allowed')).toBeTruthy()
    expect(within(row('apple-events')).getByText('Can’t tell')).toBeTruthy()

    // Allowed rows have nothing to press; the rest open System Settings.
    expect(within(row('microphone')).queryByRole('button')).toBeNull()
    expect(within(row('accessibility')).queryByRole('button')).toBeNull()
    expect(within(row('full-disk-access')).getByRole('button').textContent).toBe('Open System Settings')
    expect(within(row('local-network')).getByRole('button').textContent).toBe('Open System Settings')

    // Can't tell is explained once, under the card, not on every row.
    expect(screen.getByTestId('permissions-cant-tell').textContent).toMatch(/doesn.t let apps check Automation or Local Network/)
    expect(screen.queryByText(/TCC|Fn-Fn|Programmatic|Apple Events/)).toBeNull()
    expect(document.body.textContent).not.toMatch(/—/)
  })

  it('offers Allow… for a microphone that is not allowed, and says when macOS is asking', async () => {
    status = { fullDiskAccess: false, accessibility: false, microphone: false }
    requestReply = { granted: false, openedSettings: false }
    render(<PermissionsSection />)
    const button = await waitFor(() => within(row('microphone')).getByRole('button', { name: 'Allow…' }))
    fireEvent.click(button)
    await waitFor(() => expect(within(row('microphone')).getByRole('status').textContent).toMatch(/macOS is asking you now/))
    expect(invoke).toHaveBeenCalledWith('permissions_request_microphone')
  })

  it('opens System Settings, then flips the row to Allowed on the next poll', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    render(<PermissionsSection />)
    await waitFor(() => expect(statusOf('full-disk-access')).toBe('not-allowed'))
    fireEvent.click(within(row('full-disk-access')).getByRole('button', { name: 'Open System Settings' }))
    await waitFor(() =>
      expect(within(row('full-disk-access')).getByRole('status').textContent).toMatch(/System Settings is open at Full Disk Access/)
    )
    expect(invoke).toHaveBeenCalledWith('permissions_request_full_disk_access')

    status = { fullDiskAccess: true, accessibility: true, microphone: true }
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100)
    })
    await waitFor(() => expect(statusOf('full-disk-access')).toBe('allowed'))
    expect(within(row('full-disk-access')).queryByRole('button')).toBeNull()
    expect(within(row('full-disk-access')).getByRole('status').textContent).toMatch(/Full Disk Access is on/)
  })

  it('says once when macOS could not be read, shows Can’t tell, and tries again', async () => {
    status = new Error('permission check is not available here')
    render(<PermissionsSection />)
    const banner = await screen.findByTestId('permissions-read-failed')
    expect(banner.textContent).toMatch(/couldn.t read these permissions/)
    expect(banner.textContent).toMatch(/Reason: permission check is not available here\./)
    for (const id of ['microphone', 'full-disk-access', 'accessibility', 'apple-events', 'local-network']) {
      expect(statusOf(id)).toBe('unknown')
    }
    expect(screen.queryByTestId('permissions-cant-tell')).toBeNull()

    status = { fullDiskAccess: true, accessibility: true, microphone: true }
    fireEvent.click(within(banner).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.queryByTestId('permissions-read-failed')).toBeNull())
    expect(statusOf('microphone')).toBe('allowed')
  })

  it('says on the row when System Settings would not open', async () => {
    requestError = 'failed to open System Settings: no such app'
    render(<PermissionsSection />)
    await waitFor(() => expect(statusOf('full-disk-access')).toBe('not-allowed'))
    fireEvent.click(within(row('full-disk-access')).getByRole('button', { name: 'Open System Settings' }))
    const alert = await waitFor(() => within(row('full-disk-access')).getByRole('alert'))
    expect(alert.textContent).toMatch(/Couldn.t open System Settings\. Open it yourself, then go to Privacy & Security → Full Disk Access\./)
    expect(alert.textContent).toMatch(/Reason: failed to open System Settings: no such app\./)
  })

  it('shows a loading card until macOS answers', async () => {
    invoke.mockImplementation(() => new Promise(() => {}))
    render(<PermissionsSection />)
    expect(screen.getByTestId('permissions-loading').textContent).toMatch(/Checking permissions/)
  })

  it('keeps the search entries plain', () => {
    expect(PERMISSIONS_MANIFEST.map((e) => e.label)).toEqual([
      'Microphone',
      'Full Disk Access',
      'Accessibility',
      'Automation',
      'Local Network',
    ])
    for (const e of PERMISSIONS_MANIFEST) expect(e.description).not.toMatch(/TCC|~\/|Programmatic|LAN/)
  })
})
