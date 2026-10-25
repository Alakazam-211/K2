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
    expect(folderTrustLine(status({ state: 'clean' }))).toBe('Clean, no risky settings')
    expect(folderTrustLine(status({}))).toBe('Trusted this version by rosson on 10-24')
    expect(folderTrustLine(status({ state: 'always' }))).toBe('Always by rosson on 10-24')
    expect(folderTrustLine(status({ state: 'denied' }))).toBe('Not trusted by rosson on 10-24: K2 starts no AI tools here')
    expect(folderTrustLine(status({ state: 'pending', decided_by: null }))).toBe('Waiting for you: answer the card on Thread')
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
    await waitFor(() => expect(screen.getByTestId('folder-trust-line').textContent).toBe('Clean, no risky settings'))
    expect(screen.queryByRole('button', { name: 'Reset' })).toBeNull()
    cleanup()
    daemonCliGet.mockRejectedValueOnce(new Error('only this server’s owner answers a settings card'))
    render(<FolderTrustRow scope={scope} projectPath="/w/sales" />)
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('owner'))
  })
})
