// @vitest-environment jsdom

// Settings → Compute → "CI on K2 Compute (preview)" (0.46.17): the row reads
// `ciPreview` from the server, the owner flips it through the POST-only
// `/cli/settings/update` with exactly `{"ciPreview": <bool>}`, and anyone
// else sees it read-only.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

vi.mock('@/lib/daemon-settings', () => ({ settingsGet: vi.fn(), settingsUpdate: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn() }))

import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
import { daemonCliGet } from '@/lib/daemon-cli'
import { CI_PREVIEW_LABEL, CiPreviewRow } from './CiPreviewRow'

const get = vi.mocked(settingsGet)
const update = vi.mocked(settingsUpdate)
const whoami = vi.mocked(daemonCliGet)

function asRole(role: 'owner' | 'admin' | 'member'): void {
  whoami.mockImplementation(async (_scope: unknown, route: string) => {
    if (route !== 'auth/whoami') throw new Error(`unexpected route ${route}`)
    return { role }
  })
}

beforeEach(() => {
  get.mockReset()
  update.mockReset()
  whoami.mockReset()
  get.mockResolvedValue({ ciPreview: false } as never)
})

afterEach(() => {
  cleanup()
})

async function theSwitch(): Promise<HTMLElement> {
  return screen.findByRole('switch', { name: CI_PREVIEW_LABEL })
}

describe('CI on K2 Compute (preview)', () => {
  it('renders the server’s value with one line of explanation', async () => {
    asRole('owner')
    get.mockResolvedValue({ ciPreview: true } as never)
    render(<CiPreviewRow computeOn />)
    const sw = await theSwitch()
    await waitFor(() => expect(sw.getAttribute('aria-checked')).toBe('true'))
    const row = screen.getByTestId('ci-preview-row')
    expect(row.textContent).toContain('K2 checks every pushed commit on your compute machines')
    expect(row.textContent).toContain('other jobs may wait longer')
    expect(row.textContent).toContain('Changes take effect at once')
    expect(screen.queryByTestId('ci-preview-needs-compute')).toBeNull()
  })

  it('the owner turns it on: one POST with exactly {"ciPreview": true}', async () => {
    asRole('owner')
    update.mockResolvedValue({ ciPreview: true } as never)
    render(<CiPreviewRow computeOn />)
    const sw = await theSwitch()
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1))
    expect(update.mock.calls[0]).toEqual([{ ciPreview: true }])
    await waitFor(() => expect(sw.getAttribute('aria-checked')).toBe('true'))
    expect(await screen.findByText('Saved')).toBeTruthy()
  })

  it('a refused write puts the switch back and says why', async () => {
    asRole('owner')
    update.mockRejectedValue(new Error('settings_update 403: {"error":"updating \\"ciPreview\\" requires the Owner role"}'))
    render(<CiPreviewRow computeOn />)
    const sw = await theSwitch()
    await waitFor(() => expect((sw as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(sw)
    expect((await screen.findByRole('alert')).textContent).toContain('only this server’s owner can change this')
    expect(sw.getAttribute('aria-checked')).toBe('false')
  })

  for (const role of ['admin', 'member'] as const) {
    it(`is read-only for ${role === 'admin' ? 'an admin' : 'a member'}: disabled, nothing posted`, async () => {
      asRole(role)
      render(<CiPreviewRow computeOn />)
      const sw = await theSwitch()
      expect((await screen.findByTestId('ci-preview-owner-only')).textContent).toContain('Only this server’s owner can turn this on or off')
      expect((sw as HTMLButtonElement).disabled).toBe(true)
      fireEvent.click(sw)
      expect(update).not.toHaveBeenCalled()
    })
  }

  it('stays read-only while the viewer’s role is unknown', async () => {
    whoami.mockRejectedValue(new Error('offline'))
    render(<CiPreviewRow computeOn />)
    const sw = await theSwitch()
    await waitFor(() => expect(get).toHaveBeenCalled())
    expect((sw as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(sw)
    expect(update).not.toHaveBeenCalled()
  })

  it('says it needs compute on while compute is off', async () => {
    asRole('owner')
    render(<CiPreviewRow computeOn={false} />)
    expect((await screen.findByTestId('ci-preview-needs-compute')).textContent).toContain('Let agents use your machines')
  })
})
