// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react'

const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...args: unknown[]) => daemonCliGet(...args)),
    daemonCliPost: primaryOnly((...args: unknown[]) => daemonCliPost(...args)),
  }
})

import { primaryScope } from '@/kessel/server-scope'
import { AppDeploys, rollbackTarget } from './AppDeploys'
import { PublishedServiceDetailsModal } from './PublishedServiceDetailsModal'
import type { PublishedService } from './urls-ports'

const A = 'a'.repeat(64)
const B = 'b'.repeat(64)

function history(live: string) {
  return {
    ok: true,
    app: 'jim',
    liveSeed: live,
    auditVerified: true,
    deployments: [
      { id: 'd-2', kind: 'deploy', fromSeed: A, toSeed: B, commitRef: 'c0ffee1234567890', actor: 'agent:jimws', reason: null, result: 'live', error: null, startedAt: 2 },
      { id: 'd-1', kind: 'deploy', fromSeed: null, toSeed: A, commitRef: null, actor: 'owner', reason: null, result: 'superseded', error: null, startedAt: 1 },
    ],
  }
}

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
})

afterEach(() => {
  cleanup()
})

describe('AppDeploys', () => {
  it('lists who deployed what and rolls back through the daemon', async () => {
    daemonCliGet.mockResolvedValue(history(B))
    daemonCliPost.mockResolvedValue({ ok: true })
    render(<AppDeploys scope={primaryScope()} app="jim" />)
    await waitFor(() => expect(screen.getByText(/by agent:jimws/)).toBeTruthy())
    expect(daemonCliGet).toHaveBeenCalledWith('app/history', { app: 'jim', limit: 10 })
    expect(screen.getByText(/commit c0ffee123456/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Roll back' }))
    await waitFor(() => expect(daemonCliPost).toHaveBeenCalledWith('app/rollback', { app: 'jim', wait: 30 }))
    await waitFor(() => expect(daemonCliGet).toHaveBeenCalledTimes(2))
  })

  it('offers no Roll back with nothing earlier, and shows a daemon error', async () => {
    daemonCliGet.mockResolvedValue({ ...history(B), deployments: history(B).deployments.slice(0, 1) })
    render(<AppDeploys scope={primaryScope()} app="jim" />)
    await waitFor(() => expect(screen.getByText(/by agent:jimws/)).toBeTruthy())
    expect(screen.queryByRole('button', { name: 'Roll back' })).toBeNull()
    cleanup()
    daemonCliGet.mockRejectedValue(new Error('apps_disabled'))
    render(<AppDeploys scope={primaryScope()} app="jim" />)
    await waitFor(() => expect(screen.getByText('apps_disabled')).toBeTruthy())
  })

  it('rollbackTarget is the newest earlier Seed that went live', () => {
    expect(rollbackTarget({ liveSeed: B, auditVerified: true, deployments: history(B).deployments.map((d) => ({ ...d })) })).toBe(A)
  })
})

describe('PublishedServiceDetailsModal with an App row', () => {
  const svc = (cmd: string): PublishedService => ({
    name: 'jim',
    cmd,
    cwd: '/home/k2/.k2/apps/jim',
    port: 40000,
    expose: 'local',
    desired: 'stopped',
    status: 'running',
    pid: null,
    url: null,
    target: null,
    error: null,
    lastExitCode: null,
    kind: 'cmd',
    skinRoot: '',
    envDeprecation: null,
  })

  it('shows Deploys only for (app) rows', async () => {
    daemonCliGet.mockResolvedValue(history(B))
    render(<PublishedServiceDetailsModal service={svc('(app)')} hostLabel="This Mac" scope={primaryScope()} onClose={() => {}} />)
    await waitFor(() => expect(screen.getByText('Deploys')).toBeTruthy())
    cleanup()
    render(<PublishedServiceDetailsModal service={svc('npm start')} hostLabel="This Mac" scope={primaryScope()} onClose={() => {}} />)
    expect(screen.queryByText('Deploys')).toBeNull()
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
  })
})
