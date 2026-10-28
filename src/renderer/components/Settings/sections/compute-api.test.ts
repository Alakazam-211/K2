import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: vi.fn(),
  daemonCliGet: vi.fn(),
}))

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, scopeForHost } from '@/kessel/server-scope'
import {
  computeErrorInfo,
  computeErrorMessage,
  confirmComputeNode,
  controllerHost,
  disableLocalNode,
  drainLocalNode,
  enableLocalNode,
  fetchComputeEvents,
  fetchComputeGrants,
  fetchComputeJobs,
  fetchComputeNodes,
  fetchLocalStatus,
  formatBytes,
  formatCores,
  isComputeDisabled,
  mintEnrollCode,
  nodeDot,
  pauseComputeNode,
  pauseLocalNode,
  removeComputeNode,
  renameComputeNode,
  retireComputeNode,
  resumeLocalNode,
  revokeComputeGrant,
  setComputeGrant,
  setLocalPolicy,
  setServerPriority,
  tierCountsLine,
  tierTag,
  setWorkspaceComputeOperate,
  setWorkspaceComputeSwitch,
  validNodeName,
} from './compute-api'

const get = vi.mocked(daemonCliGet)
const post = vi.mocked(daemonCliPost)

beforeEach(() => {
  get.mockReset()
  post.mockReset()
  get.mockResolvedValue({})
  post.mockResolvedValue({ ok: true })
})

describe('controller routes go to the focused server', () => {
  it('nodes list', async () => {
    get.mockResolvedValue({ nodes: [{ name: 'mini-1' }], waitingCodes: [{ name: 'x', expiresAt: 1 }], controller: 'rosson.k2.dev' })
    const out = await fetchComputeNodes()
    expect(get).toHaveBeenCalledWith(primaryScope(), 'compute/nodes')
    expect(out.controller).toBe('rosson.k2.dev')
    expect(out.nodes).toHaveLength(1)
    expect(out.waitingCodes).toEqual([{ name: 'x', expiresAt: 1 }])
  })

  it('nodes list tolerates a body without arrays', async () => {
    get.mockResolvedValue({})
    expect(await fetchComputeNodes()).toEqual({ nodes: [], waitingCodes: [], controller: '', ticketSupported: false, criticalPath: [] })
  })

  it('grants for one node, and all grants', async () => {
    get.mockResolvedValue({ grants: [{ nodeId: 'n1' }] })
    expect(await fetchComputeGrants('mini-1')).toEqual([{ nodeId: 'n1' }])
    expect(get).toHaveBeenLastCalledWith(primaryScope(), 'compute/grants', { node: 'mini-1' })
    await fetchComputeGrants()
    expect(get).toHaveBeenLastCalledWith(primaryScope(), 'compute/grants', undefined)
  })

  it('grant set / revoke', async () => {
    await setComputeGrant('mini-1', 'ws-id', { maxJobSecs: 600, maxParallel: 2 })
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/grants/set', {
      node: 'mini-1',
      workspace: 'ws-id',
      maxJobSecs: 600,
      maxParallel: 2,
    })
    await revokeComputeGrant('mini-1', 'ws-id')
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/grants/revoke', { node: 'mini-1', workspace: 'ws-id' })
  })

  it('confirm / pause / rename / remove / enroll-code', async () => {
    await confirmComputeNode('mini-1', '123 456')
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/nodes/confirm', { node: 'mini-1', sas: '123 456' })
    await pauseComputeNode('mini-1', 'drain')
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/nodes/pause', { node: 'mini-1', action: 'drain' })
    await renameComputeNode('mini-1', 'mini-2')
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/nodes/rename', { node: 'mini-1', name: 'mini-2' })
    await removeComputeNode('mini-2')
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/nodes/remove', { node: 'mini-2' })
    await mintEnrollCode('mini-3')
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'compute/nodes/enroll-code', { name: 'mini-3' })
  })

  it('jobs and events with limits', async () => {
    get.mockResolvedValueOnce({ jobs: [{ id: 'j' }] })
    expect(await fetchComputeJobs()).toEqual([{ id: 'j' }])
    expect(get).toHaveBeenLastCalledWith(primaryScope(), 'compute/jobs', { limit: 20 })
    get.mockResolvedValueOnce({ events: [{ id: 1 }] })
    expect(await fetchComputeEvents()).toEqual([{ id: 1 }])
    expect(get).toHaveBeenLastCalledWith(primaryScope(), 'compute/events', { limit: 30 })
  })

  it('workspace switch rides agent-access/set with toggle compute', async () => {
    await setWorkspaceComputeSwitch('ws-id', true)
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'agent-access/set', { toggle: 'compute', value: true, workspace: 'ws-id' })
  })

  it('operate switch rides agent-access/set with toggle compute_operate (same as k2 compute operate)', async () => {
    await setWorkspaceComputeOperate('/w/it', false)
    expect(post).toHaveBeenLastCalledWith(primaryScope(), 'agent-access/set', { toggle: 'compute_operate', value: false, workspace: '/w/it' })
  })
})

describe('this computer routes go to the local daemon', () => {
  const local = (): ReturnType<typeof scopeForHost> => scopeForHost('local')

  it('uses the local scope, not the focused one', () => {
    expect(local()).not.toBe(primaryScope())
  })

  it('status', async () => {
    get.mockResolvedValue({ installed: false, node: null })
    await fetchLocalStatus()
    expect(get).toHaveBeenLastCalledWith(local(), 'compute/local/status')
  })

  it('pause later / pause now / drain / resume', async () => {
    await pauseLocalNode()
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/pause', {})
    await pauseLocalNode(true)
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/pause', { now: true })
    await drainLocalNode()
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/drain', {})
    await resumeLocalNode()
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/resume', {})
  })

  it('policy wraps keys in set; repo_toolchains is a boolean', async () => {
    await setLocalPolicy({ cpu_cap_percent: 60, availability: 'plugged_in' })
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/policy', {
      set: { cpu_cap_percent: 60, availability: 'plugged_in' },
    })
    await setLocalPolicy({ repo_toolchains: false })
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/policy', { set: { repo_toolchains: false } })
  })

  it('enable sends privateHome explicitly and always elevate: true', async () => {
    await enableLocalNode({ controller: 'https://rosson.k2.dev', enroll: 'ABCDE-FGHJK.0123456789abcdef', name: 'mini-1', privateHome: false })
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/enable', {
      controller: 'https://rosson.k2.dev',
      enroll: 'ABCDE-FGHJK.0123456789abcdef',
      name: 'mini-1',
      privateHome: false,
      elevate: true,
    })
    // Install only: no credential.
    await enableLocalNode({ name: 'mini-1', privateHome: true })
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/enable', { name: 'mini-1', privateHome: true, elevate: true })
    // One-click: a ticket instead of an enroll code.
    await enableLocalNode({ controller: 'https://rosson.k2.dev', ticket: 'tkt', name: 'mini-1', privateHome: true })
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/enable', {
      controller: 'https://rosson.k2.dev',
      ticket: 'tkt',
      name: 'mini-1',
      privateHome: true,
      elevate: true,
    })
  })

  it('disable sends elevate: true', async () => {
    await disableLocalNode()
    expect(post).toHaveBeenLastCalledWith(local(), 'compute/local/disable', { elevate: true })
  })
})

describe('errors and formatting', () => {
  it('reads the {error:{code,hint}} shape out of the thrown message', () => {
    const err = new Error(JSON.stringify({ error: { code: 'compute_disabled', hint: 'turn it on' } }))
    expect(computeErrorInfo(err)).toEqual({ code: 'compute_disabled', hint: 'turn it on' })
    expect(computeErrorMessage(err)).toBe('turn it on (compute_disabled)')
    expect(isComputeDisabled(err)).toBe(true)
    expect(isComputeDisabled(new Error('boom'))).toBe(false)
    expect(computeErrorMessage(new Error('plain'))).toBe('plain')
  })

  it('node names match the daemon rule', () => {
    expect(validNodeName('mini-1')).toBe(true)
    expect(validNodeName('Mini')).toBe(false)
    expect(validNodeName('')).toBe(false)
    expect(validNodeName('a'.repeat(33))).toBe(false)
  })

  it('status dots', () => {
    expect(nodeDot({ state: 'pending', online: false, controllerPause: null })).toBe('pending')
    expect(nodeDot({ state: 'active', online: true, controllerPause: 'paused' })).toBe('paused')
    expect(nodeDot({ state: 'active', online: true, controllerPause: 'draining' })).toBe('draining')
    expect(nodeDot({ state: 'active', online: true, controllerPause: null })).toBe('online')
    expect(nodeDot({ state: 'active', online: false, controllerPause: null })).toBe('offline')
  })

  it('bytes, cores and controller hosts', () => {
    expect(formatBytes(16 * 2 ** 30)).toBe('16 GB')
    expect(formatBytes(512 * 2 ** 20)).toBe('512 MB')
    expect(formatBytes(null)).toBe('—')
    expect(formatCores(8000)).toBe('8 cores')
    expect(formatCores(1500)).toBe('1.5 cores')
    expect(controllerHost('https://Rosson.k2.dev/')).toBe('rosson.k2.dev')
    expect(controllerHost('rosson.k2.dev')).toBe('rosson.k2.dev')
    expect(controllerHost('http://192.168.1.5:38471')).toBe('192.168.1.5')
  })
})

describe('Settings toggles (machines and agents)', () => {
  it('retire posts the node', async () => {
    await retireComputeNode('mini-1')
    expect(post).toHaveBeenCalledWith(primaryScope(), 'compute/nodes/retire', { node: 'mini-1' })
  })

  it('nodes list carries ticketSupported (false when absent)', async () => {
    get.mockResolvedValue({ nodes: [], waitingCodes: [], controller: 'x', ticketSupported: true })
    expect((await fetchComputeNodes()).ticketSupported).toBe(true)
    get.mockResolvedValue({ nodes: [], waitingCodes: [], controller: 'x' })
    expect((await fetchComputeNodes()).ticketSupported).toBe(false)
  })
})

describe('job tiers (0.46.5)', () => {
  it('the nodes list carries the critical path as the daemon sends it', async () => {
    const cp = [{ submitter: 'k2/7f3a/builder-ptyd', workspace: 'k2', jobs: 2 }]
    get.mockResolvedValue({ nodes: [], waitingCodes: [], controller: 'x', criticalPath: cp })
    expect((await fetchComputeNodes()).criticalPath).toEqual(cp)
  })

  it('a grant write sends allowCi beside the limits it was given', async () => {
    await setComputeGrant('mini-1', 'ws-id', { maxJobSecs: 600, maxDiskGb: 20, maxParallel: 2, maxQueued: 10, allowCi: true })
    expect(post).toHaveBeenCalledWith(primaryScope(), 'compute/grants/set', {
      node: 'mini-1',
      workspace: 'ws-id',
      maxJobSecs: 600,
      maxDiskGb: 20,
      maxParallel: 2,
      maxQueued: 10,
      allowCi: true,
    })
  })

  it('the Priority switch posts the servers-limits route with priority only', async () => {
    await setServerPriority('mini-2', 'baden.k2.dev', true)
    expect(post).toHaveBeenCalledWith(primaryScope(), 'compute/nodes/servers/limits', { node: 'mini-2', server: 'baden.k2.dev', priority: true })
  })

  it('tier tags and counts are display only', () => {
    expect([tierTag('ci'), tierTag('priority'), tierTag('normal'), tierTag('batch'), tierTag(undefined)]).toEqual(['CI', 'P', '', 'B', ''])
    expect(tierCountsLine({ ci: 1, priority: 0, normal: 3 })).toBe('CI 1 · P 0 · N 3')
    expect(tierCountsLine({ ci: 0, priority: 0, normal: 0 })).toBe('')
    expect(tierCountsLine({ ci: 0, priority: 1, normal: 0, batch: 2 })).toBe('CI 0 · P 1 · N 0 · B 2')
    expect(tierCountsLine(undefined)).toBe('')
  })
})
