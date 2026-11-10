// @vitest-environment jsdom
// Settings → Workspaces → Folder trust (prd-security-sweep-0464-v1 SS10).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))

import { FolderTrustRow, folderTrustLine, type FolderTrustStatus } from './FolderTrustRow'
import type { ServerScope } from '@/kessel/server-scope'

const scope = { isPrimary: true } as unknown as ServerScope

function status(over: Partial<FolderTrustStatus>): FolderTrustStatus {
  return {
    path: '/w/sales',
    scope: '/w/sales',
    state: 'trusted',
    decided_by: 'rosson',
    decided_at: Date.UTC(2026, 9, 24, 12) / 1000,
    findings: [{ tool: 'claude', file: '.claude/settings.json', what: 'hooks', detail: 'runs `node .claude/setup.mjs`' }],
    more: 0,
    trusted_by: [],
    decide: 'k2 workspace trust /w/sales --yes|--always|--no (owner)',
    ...over,
  }
}

beforeEach(() => {
  cleanup()
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
})

describe('folderTrustLine', () => {
  it('says each state in plain words', () => {
    expect(folderTrustLine(status({ state: 'clean' }))).toBe('No risky settings found in this folder')
    expect(folderTrustLine(status({}))).toBe('Trusted this version by rosson on 10-24')
    expect(folderTrustLine(status({ state: 'always' }))).toBe('Always trusted by rosson on 10-24')
    expect(folderTrustLine(status({ state: 'denied' }))).toBe('Not trusted by rosson on 10-24: K2 starts no programs here')
    expect(folderTrustLine(status({ state: 'pending', decided_by: null }))).toBe('Waiting for you: choose here or on the card in Thread')
    expect(folderTrustLine(status({ state: 'trusted', decided_by: 'upgrade' }))).toBe(
      'Trusted before K2 checked; K2 asks again if these settings change',
    )
  })
})

describe('FolderTrustRow', () => {
  it('shows the line and findings, and Reset posts to the daemon', async () => {
    daemonCliGet.mockResolvedValueOnce(status({}))
    daemonCliPost.mockResolvedValueOnce(status({ state: 'undecided', decided_by: null, decided_at: null }))
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).toBe('Trusted this version by rosson on 10-24'))
    expect(daemonCliGet).toHaveBeenCalledWith(scope, 'folder-trust/status', { path: '/w/sales' })
    expect(screen.getByText('.claude/settings.json')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Reset' }))
    await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).toBe('Has risky settings; K2 asks at the next start'))
    expect(daemonCliPost).toHaveBeenCalledWith(scope, 'folder-trust/reset', { path: '/w/sales' })
    expect(screen.queryByRole('button', { name: 'Reset' })).toBeNull()
  })

  it('has no Reset for a clean folder and shows a refusal', async () => {
    daemonCliGet.mockResolvedValueOnce(status({ state: 'clean', findings: [] }))
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).toBe('No risky settings found in this folder'))
    expect(screen.queryByRole('button', { name: 'Reset' })).toBeNull()
    cleanup()
    daemonCliGet.mockRejectedValueOnce(new Error('only this server’s owner answers a settings card'))
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('owner'))
  })
})

// Rosson 2026-11-05: the card's three choices in Settings too, the same
// decision on the daemon, whichever is first stands, refusals inline.
describe('FolderTrustRow choices', () => {
  const waiting = status({ state: 'pending', decided_by: null, decided_at: null, fingerprint: 'fp-1' })
  const CHOICES = ['Trust it', 'Always trust this folder', "Don't trust"]

  it('shows the card’s three choices only while the folder waits for a decision', async () => {
    for (const state of ['pending', 'changed', 'undecided'] as const) {
      cleanup()
      daemonCliGet.mockResolvedValueOnce(status({ state, fingerprint: 'fp-1' }))
      render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
      await waitFor(() => expect(screen.getByTestId('folder-trust-choices')).toBeTruthy())
      for (const label of CHOICES) expect(screen.getByRole('button', { name: label })).toBeTruthy()
    }
    for (const state of ['clean', 'trusted', 'always', 'denied'] as const) {
      cleanup()
      daemonCliGet.mockResolvedValueOnce(status({ state, fingerprint: 'fp-1' }))
      render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
      await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).not.toBe('Checking…'))
      expect(screen.queryByTestId('folder-trust-choices')).toBeNull()
    }
  })

  it('decides through the card’s route, for the version shown, and shows the result', async () => {
    daemonCliGet.mockResolvedValueOnce(waiting)
    daemonCliPost.mockResolvedValueOnce({
      ok: true,
      decision: 'always',
      message: 'Always trusted. Undo in Settings → Workspaces. Starting the 1 session K2 held here again.',
      status: status({ state: 'always', decided_by: 'rosson' }),
    })
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Always trust this folder' })).toBeTruthy())
    fireEvent.click(screen.getByRole('button', { name: 'Always trust this folder' }))
    await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).toBe('Always trusted by rosson on 10-24'))
    expect(daemonCliPost).toHaveBeenCalledWith(scope, 'folder-trust/decide', {
      path: '/w/sales',
      decision: 'always',
      fingerprint: 'fp-1',
      scope: '/w/sales',
      seen_state: 'pending',
    })
    expect(screen.getByTestId('folder-trust-notice').textContent).toBe(
      'Always trusted. Undo in Settings → Workspaces. Starting the 1 session K2 held here again.',
    )
    expect(screen.queryByTestId('folder-trust-choices')).toBeNull()
    expect(screen.getByRole('button', { name: 'Reset' })).toBeTruthy()
  })

  it('shows who decided first when the card (or another window) already did', async () => {
    daemonCliGet.mockResolvedValueOnce(waiting)
    const { DaemonHttpError } = await import('@/lib/daemon-http-error')
    const message = 'Already decided: Always trusted by baden at 2026-11-05 15:12 UTC. To change it, Reset first.'
    daemonCliPost.mockRejectedValueOnce(
      new DaemonHttpError('folder_trust_already_decided', 409, null, null, {
        error: 'folder_trust_already_decided',
        message,
        current: status({ state: 'always', decided_by: 'baden' }),
      }),
    )
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Trust it' })).toBeTruthy())
    fireEvent.click(screen.getByRole('button', { name: 'Trust it' }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe(message))
    expect(screen.getByTestId('folder-trust-line').textContent).toBe('Always trusted by baden on 10-24')
    expect(screen.queryByTestId('folder-trust-choices')).toBeNull()
  })

  it('shows a refusal for someone who may not decide, in the daemon’s words', async () => {
    daemonCliGet.mockResolvedValueOnce(waiting)
    const { DaemonHttpError } = await import('@/lib/daemon-http-error')
    const hint = "only this server's owner answers a settings card (the same people as Settings → Gardens → Local Data)"
    daemonCliPost.mockRejectedValueOnce(
      new DaemonHttpError(hint, 403, 'action_card_person_only', { code: 'action_card_person_only', hint }, {
        ok: false,
        error: { code: 'action_card_person_only', hint },
      }),
    )
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByRole('button', { name: "Don't trust" })).toBeTruthy())
    fireEvent.click(screen.getByRole('button', { name: "Don't trust" }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe(hint))
    expect(screen.getByTestId('folder-trust-line').textContent).toBe('Waiting for you: choose here or on the card in Thread')
    expect(screen.getByRole('button', { name: "Don't trust" })).toBeTruthy()
  })

  it('updates when the card decides first (re-read on focus while waiting)', async () => {
    daemonCliGet.mockResolvedValueOnce(waiting)
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByTestId('folder-trust-choices')).toBeTruthy())
    daemonCliGet.mockResolvedValueOnce(status({ state: 'always', decided_by: 'baden' }))
    window.dispatchEvent(new Event('focus'))
    await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).toBe('Always trusted by baden on 10-24'))
    expect(screen.queryByTestId('folder-trust-choices')).toBeNull()
    expect(daemonCliGet).toHaveBeenCalledTimes(2)
  })
})
