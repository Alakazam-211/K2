// @vitest-environment jsdom
//
// Settings → Email Hosting: the page reads the daemon's status reply, says
// plainly when it can't, never prints a missing version, shows the shared
// Linux note on a server that can't host mail, and confirms before it turns
// the server off. The mail API is mocked; nothing reaches a daemon.

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  fetchMailStatus: vi.fn(),
  fetchDomains: vi.fn(),
  fetchApprovals: vi.fn(),
  disableServer: vi.fn(),
  addDomain: vi.fn(),
}))

vi.mock('./email-api', async (orig) => ({
  ...(await orig<typeof import('./email-api')>()),
  fetchMailStatus: (...a: unknown[]) => h.fetchMailStatus(...a),
  fetchDomains: (...a: unknown[]) => h.fetchDomains(...a),
  fetchApprovals: (...a: unknown[]) => h.fetchApprovals(...a),
  disableServer: (...a: unknown[]) => h.disableServer(...a),
  addDomain: (...a: unknown[]) => h.addDomain(...a),
  fetchDomainDetail: () => new Promise(() => {}),
  onWindowServerMailChanged: () => () => {},
}))

import { EmailHostingSection } from './EmailHostingSection'

const RUNNING = {
  ok: true, consistent: true, supported: true, state: 'running', version: '0.16.25', pinnedVersion: '0.16.25',
  hostname: 'mail.example.com', portPlan: 'tls-alpn', enableProgress: null, lastError: null, health: null,
}

beforeEach(() => {
  for (const f of Object.values(h)) f.mockReset()
  h.fetchDomains.mockResolvedValue([])
  h.fetchApprovals.mockResolvedValue([])
})
afterEach(cleanup)

describe('EmailHostingSection', () => {
  it('shows a running server in words, with its version, and no Linux note', async () => {
    h.fetchMailStatus.mockResolvedValue(RUNNING)
    render(<EmailHostingSection />)
    expect(await screen.findByText('Running')).toBeTruthy()
    expect(screen.getByText('Version 0.16.25')).toBeTruthy()
    expect(screen.queryByTestId('linux-only-note')).toBeNull()
    expect(document.body.textContent).not.toMatch(/undefined/)
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Server', 'Domains', 'Addresses', 'Approvals'])
  })

  it('never prints a version the reply left out', async () => {
    h.fetchMailStatus.mockResolvedValue({ ...RUNNING, version: null, pinnedVersion: undefined })
    render(<EmailHostingSection />)
    await screen.findByText('Running')
    expect(screen.queryByText(/Version/)).toBeNull()
    expect(document.body.textContent).not.toMatch(/undefined/)
  })

  it('says plainly when the status reply has no status, and offers Try again', async () => {
    h.fetchMailStatus.mockResolvedValueOnce({})
    render(<EmailHostingSection />)
    expect(await screen.findByText('Couldn’t read the email server’s status')).toBeTruthy()
    h.fetchMailStatus.mockResolvedValueOnce(RUNNING)
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByText('Running')).toBeTruthy()
  })

  it('shows the shared Linux note and sample data when the server can’t host mail', async () => {
    h.fetchMailStatus.mockResolvedValue({ ...RUNNING, supported: false, state: 'not-installed', version: null, hostname: null })
    render(<EmailHostingSection />)
    const note = await screen.findByTestId('linux-only-note')
    expect(note.textContent).toMatch(/^Email hosting runs on Linux servers\./)
    expect(screen.getByRole('button', { name: 'Install email server' }).hasAttribute('disabled')).toBe(true)
    // Sample mode stays network-silent past the status read.
    expect(h.fetchDomains).not.toHaveBeenCalled()
  })

  it('asks before turning the server off, and says what happened', async () => {
    h.fetchMailStatus.mockResolvedValue(RUNNING)
    h.disableServer.mockResolvedValue({ ok: true })
    render(<EmailHostingSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Turn off…' }))
    expect(h.disableServer).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }))
    await waitFor(() => expect(h.disableServer).toHaveBeenCalledTimes(1))
    expect(await screen.findByText(/Email server turned off/)).toBeTruthy()
  })

  it('keeps a failed turn-off in its dialog with the server’s reason', async () => {
    h.fetchMailStatus.mockResolvedValue(RUNNING)
    h.disableServer.mockRejectedValue(new Error('systemctl stop timed out'))
    render(<EmailHostingSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Turn off…' }))
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }))
    expect(await screen.findByText('The email server is still on.')).toBeTruthy()
    expect(screen.getByTestId('dialog-error').textContent).toMatch(/systemctl stop timed out/)
  })

  it('says when the domain list fails instead of showing it empty', async () => {
    h.fetchMailStatus.mockResolvedValue(RUNNING)
    h.fetchDomains.mockRejectedValue(new Error('database is locked'))
    render(<EmailHostingSection />)
    fireEvent.click(await screen.findByRole('tab', { name: 'Domains' }))
    expect(await screen.findByText('Couldn’t load your domains')).toBeTruthy()
    expect(screen.queryByText('No domains yet')).toBeNull()
  })

  it('adds a domain from the dialog and opens it', async () => {
    h.fetchMailStatus.mockResolvedValue(RUNNING)
    h.addDomain.mockResolvedValue({ ok: true, domain: 'example.org', status: 'pending', records: [] })
    render(<EmailHostingSection />)
    fireEvent.click(await screen.findByRole('tab', { name: 'Domains' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Add domain' }))
    fireEvent.change(screen.getByLabelText('Domain'), { target: { value: 'Example.org' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Add domain' }).at(-1)!)
    await waitFor(() => expect(h.addDomain).toHaveBeenCalledWith('example.org'))
    expect(await screen.findByTestId('hosting-domain-back')).toBeTruthy()
  })
})
