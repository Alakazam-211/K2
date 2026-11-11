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
  projects: [] as Array<{ id: string; name: string; handle?: string }>,
}))

vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: unknown[] }) => unknown) => sel({ projects: h.projects }),
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
  h.projects = []
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
    const logsEnd = src.indexOf("title: 'Cores'")
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
  const rowsText = (container: HTMLElement): string[] =>
    Array.from(container.querySelectorAll('[data-audit-row]')).map((r) => r.textContent ?? '')
  const pick = (name: string): void => {
    fireEvent.click(screen.getByRole('radio', { name }))
  }

  it('empty events: plain empty card, no version number, no list', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('No sign-ins recorded yet')).toBeTruthy()
    })
    expect(container.textContent).not.toMatch(/0\.40\.144|v0\./)
    expect(container.querySelector('[data-load-state="empty"]')).toBeTruthy()
    expect(container.querySelectorAll('[data-audit-row]').length).toBe(0)
    expect(h.fetchUsersAudit).toHaveBeenCalledTimes(1)
    expect(h.fetchUsersAudit.mock.calls[0]?.[0]).toMatchObject({ tail: 200 })
  })

  it('404: says the server needs an update, without a version; no retry loop', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'not-found' })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('This server can’t show its sign-in history yet')).toBeTruthy()
    })
    expect(container.textContent).not.toMatch(/0\.40\.144/)
    expect(h.fetchUsersAudit).toHaveBeenCalledTimes(1)
    expect(container.querySelectorAll('[data-audit-row]').length).toBe(0)
  })

  it('a failed read says so in plain words, shows the server reason quietly, and retries', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValueOnce({ kind: 'error', message: 'audit log is locked' })
    h.fetchUsersAudit.mockResolvedValueOnce({ kind: 'ok', events: [OK], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => expect(screen.getByText('Sign-in history couldn’t load')).toBeTruthy())
    expect(screen.getByText('The server said: audit log is locked.')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(container.querySelectorAll('[data-audit-row]').length).toBe(1))
    expect(h.fetchUsersAudit).toHaveBeenCalledTimes(2)
  })

  it('records are newest first; Signed in hides failures; All shows them marked', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({
      kind: 'ok',
      events: [OLDEST, BAD, OK],
      tail: 200,
    })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(container.querySelectorAll('[data-audit-row]').length).toBe(1)
    })
    expect(screen.getByText('Access Audit')).toBeTruthy()
    expect(rowsText(container)[0]).toContain('alice')
    expect(rowsText(container)[0]).toContain('signed in')
    expect(rowsText(container)[0]).toContain('203.0.113.9')
    expect(rowsText(container)[0]).toContain('through the web address')
    expect(container.textContent).not.toContain('198.51.100.7')

    pick('All')
    const all = rowsText(container)
    expect(all.length).toBe(3)
    expect(all[0]).toContain('203.0.113.9')
    expect(all[1]).toContain('wrong username or password')
    expect(all[2]).toContain('doesn’t accept')
    expect(container.querySelectorAll('[data-audit-row][data-failed="true"]').length).toBe(2)
  })

  it('Failed sign-ins plus a person narrows to that person’s failures and says how many', async () => {
    const SAM_BAD: AuthAuditEvent = { ...BAD, ts: '2026-09-16T15:00:00.000Z', user: 'sam' }
    const SAM_LOCKED: AuthAuditEvent = { ...BAD, ts: '2026-09-16T15:01:00.000Z', user: 'sam', outcome: 'locked' }
    const SAM_OK: AuthAuditEvent = { ...OK, ts: '2026-09-16T15:02:00.000Z', user: 'sam' }
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [BAD, SAM_BAD, SAM_LOCKED, SAM_OK], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => expect(container.querySelectorAll('[data-audit-row]').length).toBe(1))
    pick('Failed sign-ins')
    expect(rowsText(container).length).toBe(3)
    fireEvent.change(screen.getByLabelText('Filter by person'), { target: { value: 'sam' } })
    const rows = rowsText(container)
    expect(rows.length).toBe(2)
    expect(rows[0]).toContain('locked out')
    expect(rows[1]).toContain('wrong username or password')
    expect(screen.getByText('2 records for “sam”, newest first.')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Filter by person'), { target: { value: 'nobody' } })
    expect(screen.getByTestId('audit-no-match')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Show all records' }))
    expect(rowsText(container).length).toBe(4)
  })

  it('opening a record lists plain labels, with the raw values quietly after', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [OK], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => expect(container.querySelectorAll('[data-audit-row]').length).toBe(1))
    const toggle = container.querySelector('[data-audit-row] button') as HTMLButtonElement
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    const details = container.querySelector('[data-audit-details]') as HTMLElement
    expect(details.textContent).toContain('2026-09-16 14:02:11 UTC')
    expect(details.textContent).toContain('Came in through')
    expect(details.textContent).toContain('The web address · edge:k2-edge-app-2026-09')
    expect(details.textContent).toContain('Sign-in · login')
    expect(details.textContent).not.toMatch(/\bRoute\b|\bRecord\b/)
    // A plain sign-in has nothing more to say: no log line.
    expect(details.textContent).not.toContain('Log line')
  })

  it('an agent vault export is marked, shown under Signed in, and bannered; a person export is not', async () => {
    const AGENT_EXPORT: AuthAuditEvent = {
      ts: '2026-10-17T09:00:00.000Z',
      event: 'vault-carry',
      user: 'agent:ws-sales/sess-9',
      outcome: 'ok export 2 [server:STRIPE_KEY, server:OPENAI_KEY] of workspaces [] by agent:ws-sales/sess-9 (workspace ws-sales) to fp1 (vault v1)',
      ingress: '-',
      ip: '-',
    }
    const OWNER_EXPORT: AuthAuditEvent = { ...AGENT_EXPORT, ts: '2026-10-17T08:00:00.000Z', user: 'owner', outcome: 'ok export 1 [server:X] by owner' }
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [OK, OWNER_EXPORT, AGENT_EXPORT], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(container.querySelectorAll('[data-audit-row]').length).toBeGreaterThan(0)
    })
    // Default filter (Signed in) still shows the agent export, not the owner's.
    const flagged = container.querySelectorAll('[data-audit-row][data-agent-vault-export="true"]')
    expect(flagged.length).toBe(1)
    expect(rowsText(container).length).toBe(2)
    expect(flagged[0]?.textContent).toContain('ws-sales agent exported secrets from this server’s vault')
    expect(flagged[0]?.textContent).toContain('STRIPE_KEY and OPENAI_KEY, copied out to another K2 server')
    // The raw line and the session id stay in the opened record.
    expect(flagged[0]?.textContent).not.toContain('sess-9')
    expect(container.textContent).not.toContain(AGENT_EXPORT.outcome)
    fireEvent.click(flagged[0]!.querySelector('button')!)
    const details = container.querySelector('[data-audit-details]') as HTMLElement
    expect(details.textContent).toContain('Session')
    expect(details.textContent).toContain('sess-9')
    expect(details.textContent).toContain('Log line')
    expect(details.textContent).toContain(AGENT_EXPORT.outcome)
    expect(screen.getByTestId('agent-vault-export-banner').textContent).toContain('An agent exported secrets')
    // All: the owner's export appears, unflagged, as "You".
    pick('All')
    expect(rowsText(container).some((t) => t.startsWith('You exported secrets') && t.includes('X, copied out'))).toBe(true)
    expect(container.querySelectorAll('[data-audit-row][data-agent-vault-export="true"]').length).toBe(1)
  })

  it('an agent stamped with a workspace id is named by that workspace', async () => {
    h.projects = [{ id: 'p2', name: 'release-bot', handle: 'release-bot' }]
    const SEND: AuthAuditEvent = { ts: '2026-10-17T09:00:00.000Z', event: 'vault-handoff', user: 'agent:p2/s4', outcome: 'ok send GITHUB_TOKEN to atlas hd_0000sample pending', ingress: '-', ip: '-' }
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [OK, SEND], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => expect(container.querySelectorAll('[data-audit-row]').length).toBe(1))
    pick('Vault handoffs')
    expect(rowsText(container)[0]).toContain('release-bot agent sent a secret')
    expect(rowsText(container)[0]).not.toContain('p2')
    // The person filter finds it by the name people see.
    fireEvent.change(screen.getByLabelText('Filter by person'), { target: { value: 'release' } })
    expect(rowsText(container).length).toBe(1)
  })

  it('no agent export: no banner', async () => {
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [OK], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(container.querySelectorAll('[data-audit-row]').length).toBe(1)
    })
    expect(screen.queryByTestId('agent-vault-export-banner')).toBeNull()
  })

  it('Vault handoffs shows only vault-handoff lines, with their long outcomes', async () => {
    const SEND: AuthAuditEvent = {
      ts: '2026-10-17T09:00:00.000Z',
      event: 'vault-handoff',
      user: 'agent:ws-sales/s9',
      outcome: 'ok send STRIPE_KEY to ops hd_7f3c0000000000000000000000000000 pending',
      ingress: '-',
      ip: '-',
      client: 'vault',
    }
    const DENY: AuthAuditEvent = { ...SEND, ts: '2026-10-17T09:01:00.000Z', outcome: 'deny send LOGIN to ops: never_sendable' }
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [OK, SEND, DENY], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => expect(container.querySelectorAll('[data-audit-row]').length).toBe(1))
    // Signed in (the default) is sign-ins only.
    expect(container.textContent).not.toContain('STRIPE_KEY')
    pick('Vault handoffs')
    expect(screen.getByRole('radio', { name: 'Vault handoffs' }).getAttribute('aria-checked')).toBe('true')
    const rows = rowsText(container)
    expect(rows.length).toBe(2)
    expect(rows[0]).toContain('ws-sales agent was refused a secret handoff')
    expect(rows[0]).toContain('LOGIN to ops · Refused: this secret can never be sent')
    expect(rows[1]).toContain('ws-sales agent sent a secret')
    expect(screen.getByText('STRIPE_KEY to ops · waiting for them to accept').className).toContain('break-words')
    // Never the raw line on a row.
    expect(container.textContent).not.toContain(SEND.outcome)
    expect(container.textContent).not.toContain('hd_7f3c')
    expect(container.textContent).not.toContain('203.0.113.9')
    pick('All')
    expect(container.textContent).toContain('203.0.113.9')
  })

  it('Permission cards shows only the five permission-* lines (asks, answers, notes, undo, switches)', async () => {
    const line = (event: string, outcome: string, i: number): AuthAuditEvent => ({
      ts: `2026-10-21T10:0${i}:00.000Z`,
      event,
      user: i === 0 ? 'agent:ws-billing/s1' : 'alex',
      outcome,
      ingress: '-',
      ip: '-',
    })
    const REQ = line('permission-request', 'pr_7f3a2c compute.use pending via=passport', 0)
    const ANS = line('permission-answer', 'pr_7f3a2c compute.use approved via=login', 1)
    const NOTE = line('permission-note', 'pr_7f3a2c compute.use approved via=login note=written', 2)
    const UNDO = line('permission-undo', 'pr_7f3a2c compute.use approved via=login twin=compute.use-off undo=applied', 3)
    const SWITCH = line('permission-switch', 'server on', 4)
    const LOOKALIKE = line('permissions-sync', 'not a card line', 5)
    const HANDOFF: AuthAuditEvent = { ...REQ, ts: '2026-10-21T10:06:00.000Z', event: 'vault-handoff', outcome: 'ok send KEY to ops' }
    h.fetchWhoamiRole.mockResolvedValue('owner')
    h.fetchUsersAudit.mockResolvedValue({ kind: 'ok', events: [OK, REQ, ANS, NOTE, UNDO, SWITCH, LOOKALIKE, HANDOFF], tail: 200 })
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => expect(container.querySelectorAll('[data-audit-row]').length).toBe(1))
    expect(rowsText(container).length).toBe(1)
    pick('Permission cards')
    const rows = rowsText(container)
    expect(rows.length).toBe(5)
    // Newest first: switch, undo, note, answer, request. Plain words, no request ids.
    expect(rows[0]).toContain('Permission cards turned on for this server')
    expect(rows[1]).toContain('Turned off again: ws-billing agent’s request to run jobs on compute nodes')
    expect(rows[2]).toContain('Wrote a note on ws-billing agent’s request to run jobs on compute nodes')
    expect(rows[3]).toContain('alex answered a permission request')
    expect(rows[3]).toContain('Allowed ws-billing agent to run jobs on compute nodes')
    expect(rows[4]).toContain('ws-billing agent asked for a permission')
    expect(rows[4]).toContain('Run jobs on compute nodes · waiting for an answer')
    expect(container.textContent).not.toContain('pr_7f3a2c')
    expect(container.textContent).not.toContain('via=')
    expect(container.textContent).not.toContain('203.0.113.9')
    // A switch's state is not a failure.
    expect(container.querySelectorAll('[data-audit-row][data-failed="true"]').length).toBe(0)
    expect(ACCESS_AUDIT_MANIFEST[0]?.keywords).toContain('permission cards')
  })

  it('admin: owner-only note and no audit fetch', async () => {
    h.fetchWhoamiRole.mockResolvedValue('admin')
    const { container } = render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('Only the server’s owner can see this')).toBeTruthy()
    })
    expect(h.fetchWhoamiRole).toHaveBeenCalled()
    expect(h.fetchUsersAudit).not.toHaveBeenCalled()
    expect(container.querySelectorAll('[data-audit-row]').length).toBe(0)
  })

  it('feature gate: remote 0.40.143 does not fetch', async () => {
    h.supportsAudit.value = false
    render(<AccessAuditSection />)
    await waitFor(() => {
      expect(screen.getByText('This server can’t show its sign-in history yet')).toBeTruthy()
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
