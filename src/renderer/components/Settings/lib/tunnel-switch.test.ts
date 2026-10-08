import { describe, expect, it } from 'vitest'
import {
  runTunnelSwitch,
  type TunnelSwitchDeps,
  type TunnelSwitchProgress,
  type TunnelSwitchReport,
} from './tunnel-switch'

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function okReport(overrides: Partial<TunnelSwitchReport> = {}): TunnelSwitchReport {
  return {
    ok: true,
    changed: true,
    previousSubdomain: 'afs',
    subdomain: 'k2n1-m4',
    steps: [
      { step: 'config', ok: true, detail: 'bound k2n1-m4.k2.dev (was afs.k2.dev)' },
      { step: 'listener', ok: true, detail: 'stopped the encrypted listener and certificate renewer for afs.k2.dev' },
      { step: 'certificate', ok: true, detail: 'certificate and encrypted listener ready for k2n1-m4.k2.dev (port 50123)' },
      { step: 'tunnel', ok: true, detail: 'tunnel connected as https://k2n1-m4.k2.dev' },
    ],
    rolledBack: false,
    ...overrides,
  }
}

interface Recorder {
  calls: string[]
  progress: TunnelSwitchProgress[]
  bodies: Record<string, unknown>[]
}

function deps(
  rec: Recorder,
  opts: {
    claim?: { claimed: boolean; holder: string | null } | Error | null
    response?: Response | Error
  },
): TunnelSwitchDeps {
  return {
    claim:
      opts.claim === null
        ? null
        : async (label) => {
            rec.calls.push(`claim:${label}`)
            if (opts.claim instanceof Error) throw opts.claim
            return opts.claim ?? { claimed: true, holder: 'dev-me' }
          },
    releaseLease: async (label) => {
      rec.calls.push(`release:${label}`)
    },
    postSwitch: async (body) => {
      rec.calls.push('switch')
      rec.bodies.push(body)
      if (opts.response instanceof Error) throw opts.response
      return opts.response ?? jsonResponse(200, okReport())
    },
  }
}

function recorder(): Recorder {
  return { calls: [], progress: [], bodies: [] }
}

const request = {
  from: 'afs',
  to: 'k2n1-m4',
  token: 'tok-new',
  deviceId: 'dev-me',
  deviceLabel: 'MacIntel',
  start: true,
}

describe('runTunnelSwitch', () => {
  it('claims the new name first, lets the daemon switch, then releases the old lease', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(request, deps(rec, {}), (p) => rec.progress.push(p))
    expect(out.ok).toBe(true)
    expect(out.error).toBeNull()
    expect(rec.calls).toEqual(['claim:k2n1-m4', 'switch', 'release:afs'])
    expect(rec.bodies).toEqual([
      { subdomain: 'k2n1-m4', token: 'tok-new', deviceId: 'dev-me', deviceLabel: 'MacIntel', start: true },
    ])
    expect(rec.progress.map((p) => p.phase)).toEqual(['claim', 'daemon', 'done'])
    const done = rec.progress[2]
    expect(done.message).toBe('Now connected as k2n1-m4.k2.dev.')
    expect(done.steps.map((s) => s.step)).toEqual(['config', 'listener', 'certificate', 'tunnel'])
  })

  it('a claim held by another device changes nothing on this device', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(
      request,
      deps(rec, { claim: { claimed: false, holder: 'dev-other' } }),
      (p) => rec.progress.push(p),
    )
    expect(out).toMatchObject({ ok: false, unchanged: true, rolledBack: false })
    expect(out.error).toBe('k2n1-m4.k2.dev is in use on another device — stop it there first.')
    expect(rec.calls).toEqual(['claim:k2n1-m4'])
    expect(rec.progress.map((p) => p.phase)).toEqual(['claim', 'failed'])
  })

  it('a failed claim request changes nothing', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(request, deps(rec, { claim: new Error('session expired') }), () => {})
    expect(out).toMatchObject({ ok: false, unchanged: true, error: 'session expired' })
    expect(rec.calls).toEqual(['claim:k2n1-m4'])
  })

  it('a daemon rollback is reported with the old name and frees the new lease', async () => {
    const rec = recorder()
    const report = okReport({
      ok: false,
      rolledBack: true,
      subdomain: 'afs',
      error: 'cert broker rejected the tunnel token (invalid_token): bad',
      steps: [
        { step: 'config', ok: true, detail: 'bound k2n1-m4.k2.dev (was afs.k2.dev)' },
        { step: 'certificate', ok: false, detail: 'cert broker rejected the tunnel token (invalid_token): bad' },
        { step: 'rollback', ok: true, detail: 'restored afs.k2.dev and its tunnel' },
      ],
    })
    const out = await runTunnelSwitch(
      request,
      deps(rec, { response: jsonResponse(200, report) }),
      (p) => rec.progress.push(p),
    )
    expect(out.ok).toBe(false)
    expect(out.rolledBack).toBe(true)
    expect(out.error).toBe(
      'Switch to k2n1-m4.k2.dev failed: cert broker rejected the tunnel token (invalid_token): bad. afs.k2.dev was put back.',
    )
    expect(rec.calls).toEqual(['claim:k2n1-m4', 'switch', 'release:k2n1-m4'])
    const failed = rec.progress[rec.progress.length - 1]
    expect(failed.phase).toBe('failed')
    expect(failed.steps.map((s) => `${s.step}:${s.ok}`)).toEqual(['config:true', 'certificate:false', 'rollback:true'])
  })

  it('a 400 from the daemon (bad input) shows its error and frees the new lease', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(
      request,
      deps(rec, { response: jsonResponse(400, { error: 'invalid subdomain "X"' }) }),
      () => {},
    )
    expect(out).toMatchObject({ ok: false, unchanged: true, error: 'invalid subdomain "X"' })
    expect(rec.calls).toEqual(['claim:k2n1-m4', 'switch', 'release:k2n1-m4'])
  })

  it('a daemon that does not answer is an error, not a hang', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(request, deps(rec, { response: new Error('Failed to fetch') }), () => {})
    expect(out).toMatchObject({ ok: false, error: 'Failed to fetch' })
    expect(rec.calls).toEqual(['claim:k2n1-m4', 'switch', 'release:k2n1-m4'])
  })

  it('without an account session the daemon still switches (nothing to claim)', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(request, deps(rec, { claim: null }), (p) => rec.progress.push(p))
    expect(out.ok).toBe(true)
    expect(rec.calls).toEqual(['switch', 'release:afs'])
    expect(rec.progress.map((p) => p.phase)).toEqual(['daemon', 'done'])
  })

  it('binding after a release (no current name, tunnel off) releases nothing', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch(
      { ...request, from: '', start: false },
      deps(rec, { response: jsonResponse(200, okReport({ previousSubdomain: null })) }),
      (p) => rec.progress.push(p),
    )
    expect(out.ok).toBe(true)
    expect(rec.calls).toEqual(['claim:k2n1-m4', 'switch'])
    expect(rec.bodies[0].start).toBe(false)
    expect(rec.progress[rec.progress.length - 1].message).toBe('Bound k2n1-m4.k2.dev — Start the tunnel below.')
  })

  it('switching to the same name (repair) keeps its lease', async () => {
    const rec = recorder()
    const out = await runTunnelSwitch({ ...request, from: 'k2n1-m4' }, deps(rec, {}), () => {})
    expect(out.ok).toBe(true)
    expect(rec.calls).toEqual(['claim:k2n1-m4', 'switch'])
  })
})
