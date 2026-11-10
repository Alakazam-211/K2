import { beforeEach, describe, expect, it, vi } from 'vitest'
import * as daemonCli from '@/lib/daemon-cli'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { primaryScope } from '@/kessel/server-scope'
import { cliStatusLabel, fetchCliStatus, type CliStatus } from '@/lib/cli-status'

vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn() }))

const get = vi.mocked(daemonCli.daemonCliGet)

function row(over: Partial<CliStatus> & Pick<CliStatus, 'program'>): CliStatus {
  return { installed: false, version: null, path: null, launches: false, error: null, ...over }
}

describe('fetchCliStatus', () => {
  beforeEach(() => {
    get.mockReset()
  })

  it('asks GET agents/cli-status and maps the rows by program', async () => {
    const claude = row({ program: 'claude', installed: true, version: '2.0.14', path: '/u/.local/bin/claude', launches: true })
    get.mockResolvedValue({ ok: true, cacheSecs: 60, tools: [claude, row({ program: 'gemini' })] })
    const r = await fetchCliStatus(primaryScope())
    expect(get).toHaveBeenCalledWith(primaryScope(), 'agents/cli-status')
    if (r.kind !== 'ready') throw new Error(`expected ready, got ${r.kind}`)
    expect(r.byProgram.get('claude')).toEqual(claude)
    expect(r.byProgram.get('gemini')?.installed).toBe(false)
    expect(r.byProgram.has('cursor-agent')).toBe(false)
  })

  it('is unsupported on an older daemon without the route (404)', async () => {
    get.mockRejectedValue(new DaemonHttpError('route not found', 404, null))
    expect(await fetchCliStatus(primaryScope())).toEqual({ kind: 'unsupported' })
    get.mockRejectedValue(new DaemonHttpError('route_unclassified', 404, null))
    expect(await fetchCliStatus(primaryScope())).toEqual({ kind: 'unsupported' })
  })

  it('is an error for any other failure or an unexpected body', async () => {
    get.mockRejectedValue(new DaemonHttpError('invalid or missing token', 403, null))
    expect(await fetchCliStatus(primaryScope())).toEqual({ kind: 'error', message: 'invalid or missing token' })
    get.mockRejectedValue(new Error('connection refused'))
    expect(await fetchCliStatus(primaryScope())).toEqual({ kind: 'error', message: 'connection refused' })
    get.mockResolvedValue({ ok: true, tools: [{ program: 'claude', installed: 'yes' }] })
    expect((await fetchCliStatus(primaryScope())).kind).toBe('error')
    get.mockResolvedValue({ ok: true })
    expect((await fetchCliStatus(primaryScope())).kind).toBe('error')
  })
})

describe('cliStatusLabel', () => {
  it('names the three states', () => {
    expect(cliStatusLabel(row({ program: 'gemini' }))).toBe('Not installed')
    expect(cliStatusLabel(row({ program: 'claude', installed: true, version: '2.0.14', launches: true }))).toBe('Installed · 2.0.14')
    expect(cliStatusLabel(row({ program: 'codex', installed: true, error: '--version exited 1' }))).toBe('Installed, not working')
  })
})
