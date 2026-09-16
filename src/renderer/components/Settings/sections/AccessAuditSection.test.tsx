// @vitest-environment jsdom

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({
  fetchUsersAudit: vi.fn(),
  fetchWhoamiRole: vi.fn(),
  supportsAudit: { value: true },
}))

vi.mock('./access-audit-api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./access-audit-api')>()
  return {
    ...actual,
    fetchUsersAudit: h.fetchUsersAudit,
    fetchWhoamiRole: h.fetchWhoamiRole,
  }
})

vi.mock('@/lib/server-capabilities', () => ({
  useServerSupports: () => h.supportsAudit.value,
  serverSupports: () => h.supportsAudit.value,
  FEATURES: { 'users-audit': '0.40.144' },
}))

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

import { AccessAuditSection, ACCESS_AUDIT_MANIFEST } from './AccessAuditSection'
import { useConnectHostStore } from '@/stores/connect-host'
import type { AuthAuditEvent } from './access-audit-api'

const OLDEST: AuthAuditEvent = {
  ts: '2026-09-16T13:55:02.000Z',
  event: 'login',
  user: 'root',
  outcome: 'blocked_ingress',
  ingress: 'tunnel',
  ip: '-',
}
const BAD: AuthAuditEvent = {
  ts: '2026-09-16T14:01:58.000Z',
  event: 'login',
  user: 'alice',
  outcome: 'bad_creds',
  ingress: 'edge:k2-edge-app-2026-09',
  ip: '198.51.100.7',
}
const OK: AuthAuditEvent = {
  ts: '2026-09-16T14:02:11.000Z',
  event: 'login',
  user: 'alice',
  outcome: 'ok',
  ingress: 'edge:k2-edge-app-2026-09',
  ip: '203.0.113.9',
}

beforeEach(() => {
  cleanup()
  h.fetchUsersAudit.mockReset()
  h.fetchWhoamiRole.mockReset()
  h.supportsAudit.value = true
  useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
})

describe('ACCESS_AUDIT_MANIFEST', () => {
  it('search id is k2-connect.access-audit with login-history keywords', () => {
    expect(ACCESS_AUDIT_MANIFEST[0]?.id).toBe('k2-connect.access-audit')
    expect(ACCESS_AUDIT_MANIFEST[0]?.section).toBe('access-audit')
    expect(ACCESS_AUDIT_MANIFEST[0]?.label).toBe('Access Audit')
    const keys = ACCESS_AUDIT_MANIFEST[0]?.keywords ?? []
    expect(keys).toEqual(
      expect.arrayContaining(['access audit', 'login history', 'audit', 'sign-in', 'compromised']),
    )
  })
})

describe('Logs nav', () => {
  it('includes Access Audit after Heartbeats/Timer in Settings.tsx blocks', () => {
    const src = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../Settings.tsx'), 'utf8')
    const logsStart = src.indexOf("title: 'Logs'")
    const logsEnd = src.indexOf("title: 'Sidecars'")
    expect(logsStart).toBeGreaterThan(0)
    expect(logsEnd).toBeGreaterThan(logsStart)
    const logs = src.slice(logsStart, logsEnd)
    expect(logs).toContain("id: 'wake-scheduler', label: 'Heartbeats'")
    expect(logs).toContain("id: 'timer', label: 'Timer'")
    expect(logs).toContain("id: 'access-audit', label: 'Access Audit'")
    expect(logs.indexOf("id: 'timer'")).toBeLessThan(logs.indexOf("id: 'access-audit'"))
    expect(src).toContain('<AccessAuditSection />')
    expect(src).toContain("activeSection === 'access-audit'")
  })
})

describe('AccessAuditSection', () => {
  it('empty events: H10 copy, no table', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [], tail: 200 })
    render(<AccessAuditSection />)
    await waitFor(() => {
      expect(
        screen.getByText(/No login records yet on this host. Logging started in 0.40.144/),
      ).toBeTruthy()
    })
    expect(screen.queryByRole('table')).toBeNull()
    expect(h.fetchUsersAudit).toHaveBeenCalledTimes(1)
    expect(h.fetchUsersAudit.mock.calls[0]?.[0]).toMatchObject({ tail: 200 })
  })

  it('404: old-host copy; no retry loop', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'not-found' })
    render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('This host predates login history — update it to v0.40.144.')).toBeTruthy()
    })
    expect(h.fetchUsersAudit).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('table')).toBeNull()
  })

  it('owner table is newest-first; Successes hides bad_creds', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({
      kind: 'ok',
      events: [OLDEST, BAD, OK],
      tail: 200,
    })
    render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByRole('table')).toBeTruthy()
    })
    expect(screen.getByText('Access Audit')).toBeTruthy()
    expect(screen.getByText('203.0.113.9')).toBeTruthy()
    expect(screen.queryByText('bad_creds')).toBeNull()
    expect(screen.queryByText('blocked_ingress')).toBeNull()
    expect(screen.queryByText('198.51.100.7')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'All' }))
    const cells = screen.getAllByRole('cell').map((c) => c.textContent)
    const firstUserRow = cells.slice(0, 6)
    expect(firstUserRow.join(' ')).toContain('alice')
    expect(firstUserRow.join(' ')).toContain('ok')
    expect(firstUserRow.join(' ')).toContain('203.0.113.9')
    expect(screen.getByText('bad_creds')).toBeTruthy()
    expect(screen.getByText('blocked_ingress')).toBeTruthy()
    const ipCells = screen.getAllByText(/203\.0\.113\.9|198\.51\.100\.7|^-$/)
    expect(ipCells[0]?.textContent).toBe('203.0.113.9')
  })

  it('admin: owner-only note and no audit fetch', async () => {
    h.fetchWhoamiRole.mockResolvedValue('admin')
    render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('Login history is visible to the server owner.')).toBeTruthy()
    })
    expect(h.fetchWhoamiRole).toHaveBeenCalled()
    expect(h.fetchUsersAudit).not.toHaveBeenCalled()
    expect(screen.queryByRole('table')).toBeNull()
  })

  it('feature gate: remote 0.40.143 does not fetch', async () => {
    h.supportsAudit.value = false
    render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('This host predates login history — update it to v0.40.144.')).toBeTruthy()
    })
    expect(h.fetchWhoamiRole).not.toHaveBeenCalled()
    expect(h.fetchUsersAudit).not.toHaveBeenCalled()
  })

  it('deep-link data-settings-id is access-audit', async () => {
    h.fetchWhoamiRole.mockResolvedValue('admin')
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(container.querySelector('[data-settings-id="access-audit"]')).toBeTruthy()
    })
    expect(container.querySelector('[data-settings-id="k2-connect.access-audit"]')).toBeTruthy()
  })
})
