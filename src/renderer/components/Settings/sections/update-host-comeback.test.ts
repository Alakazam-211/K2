// Remote update across the restart (0.46.3, Rosson 2026-10-19): updating a
// remote server from Settings → General said "failed" although the server
// updated. The apply ack was lost when the daemon cut its own tunnel, and
// the row treated the dropped connection as the verdict. These tests lock
// the replacement rules: a dropped connection mid-update is never a failure
// by itself; the verdict comes from what the host reports once it is back.

import { describe, it, expect } from 'vitest'
import {
  isAmbiguousApplyError,
  pollErrorMeansRestart,
  watchUpdateComeback,
  updateReconnectingCopy,
  updateSucceededCopy,
  updateFailedCopy,
  updateUnconfirmedCopy,
  type UpdateComebackDeps,
  type UpdateStatusResult,
} from './update-host'

/** An HTTP error shaped like DaemonHttpError / HostOpError. */
function httpErr(status: number, message: string): Error {
  return Object.assign(new Error(message), { status })
}

type Boot = Awaited<ReturnType<UpdateComebackDeps['bootStatus']>>

/** Scripted host: each tick consumes the next boot answer (the last one
 *  repeats) and the matching job answer. A virtual clock advances by the
 *  sleep interval, so the 4/10-minute deadlines run instantly. */
function scriptedHost(
  boots: Boot[],
  jobs: Array<UpdateStatusResult | null> = [],
): { deps: UpdateComebackDeps; ticks: () => number; slowCalls: () => number } {
  let t = 0
  let tick = -1
  let slow = 0
  let jobCalls = 0
  const deps: UpdateComebackDeps = {
    bootStatus: async () => {
      tick++
      return boots[Math.min(tick, boots.length - 1)]
    },
    jobStatus: async () => {
      const j = jobs.length === 0 ? null : jobs[Math.min(jobCalls, jobs.length - 1)]
      jobCalls++
      return j
    },
    sleep: async (ms) => {
      t += ms
    },
    now: () => t,
    alive: () => true,
    onSlow: () => {
      slow++
    },
  }
  return { deps, ticks: () => tick + 1, slowCalls: () => slow }
}

const OLD = '0.46.2'
const NEW = '0.46.3'
const OPTS = { expected: NEW, from: OLD, startInstanceId: 'aaaa', intervalMs: 2500 }

describe('isAmbiguousApplyError — apply was sent, the answer got lost', () => {
  it('a dropped connection is the restart, not a refusal (the reported bug: "Load failed")', () => {
    expect(isAmbiguousApplyError(new TypeError('Load failed'))).toBe(true)
    expect(isAmbiguousApplyError(new TypeError('Failed to fetch'))).toBe(true)
    expect(isAmbiguousApplyError(new Error('The request timed out.'))).toBe(true)
  })

  it('a relay 502/503/504 while the tunnel is down is the restart', () => {
    expect(isAmbiguousApplyError(httpErr(502, 'Bad Gateway'))).toBe(true)
    expect(isAmbiguousApplyError(httpErr(503, 'migrating'))).toBe(true)
    expect(isAmbiguousApplyError(httpErr(504, 'Gateway Timeout'))).toBe(true)
  })

  it('a retried apply that reached the NEW daemon is the restart', () => {
    expect(
      isAmbiguousApplyError(httpErr(400, "job abc is 'done', not 'staged' — cannot apply")),
    ).toBe(true)
    expect(isAmbiguousApplyError(httpErr(400, 'unknown job_id abc'))).toBe(true)
  })

  it('a real refusal from a daemon still up on the old version is NOT ambiguous', () => {
    expect(isAmbiguousApplyError(httpErr(500, 'swap binary: Permission denied (os error 13)'))).toBe(false)
    expect(isAmbiguousApplyError(httpErr(500, 'back up running binary: No space left on device'))).toBe(false)
    expect(isAmbiguousApplyError(httpErr(403, 'invalid or missing token'))).toBe(false)
    expect(isAmbiguousApplyError(httpErr(400, 'invalid body: expected value'))).toBe(false)
  })
})

describe('pollErrorMeansRestart — when a status-poll error means "installing"', () => {
  const drop = new TypeError('Load failed')

  it('standalone: only after apply was sent', () => {
    expect(pollErrorMeansRestart({ err: drop, installKind: 'standalone', applySent: false })).toBe(false)
    expect(pollErrorMeansRestart({ err: drop, installKind: 'standalone', applySent: true })).toBe(true)
  })

  it('bundled-app: any time after start (the app stops the daemon right after posting restarting)', () => {
    expect(pollErrorMeansRestart({ err: drop, installKind: 'bundled-app', applySent: false })).toBe(true)
  })

  it('a 404 for the job (pre-0.46.3 daemon restarted and forgot it) or a gateway status counts', () => {
    expect(pollErrorMeansRestart({ err: httpErr(404, 'route not found'), installKind: 'bundled-app', applySent: false })).toBe(true)
    expect(pollErrorMeansRestart({ err: httpErr(502, 'Bad Gateway'), installKind: 'standalone', applySent: true })).toBe(true)
  })

  it('a 403 never does', () => {
    expect(pollErrorMeansRestart({ err: httpErr(403, 'forbidden'), installKind: 'bundled-app', applySent: true })).toBe(false)
  })
})

describe('watchUpdateComeback — the verdict comes from the host, not the dropped connection', () => {
  it('connection drops mid-update, then the host reconnects on the new version → success', async () => {
    const host = scriptedHost([
      null, // daemon / tunnel down
      null,
      { phase: 'starting', version: NEW, instanceId: 'bbbb' },
      { phase: 'ready', version: NEW, instanceId: 'bbbb' },
    ])
    const out = await watchUpdateComeback(host.deps, OPTS)
    expect(out).toEqual({ kind: 'success', version: NEW })
    expect(host.ticks()).toBe(4)
  })

  it('success on the new version even with no outage seen (hand-off kept the socket up)', async () => {
    const host = scriptedHost([{ phase: 'ready', version: NEW, instanceId: 'bbbb' }])
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({ kind: 'success', version: NEW })
  })

  it('the daemon’s persisted job says done → success (version from /boot-status)', async () => {
    const host = scriptedHost(
      [null, { phase: 'ready', version: NEW, instanceId: 'bbbb' }],
      [{ phase: 'done', target_version: NEW, from_version: OLD, running_version: NEW }],
    )
    // Expected unknown (e.g. check result lost): the daemon's verdict decides.
    const out = await watchUpdateComeback(host.deps, { ...OPTS, expected: undefined })
    expect(out).toEqual({ kind: 'success', version: NEW })
  })

  it('the host restarts and comes back on the OLD version → failed, saying so', async () => {
    const host = scriptedHost([null, { phase: 'ready', version: OLD, instanceId: 'bbbb' }])
    const out = await watchUpdateComeback(host.deps, OPTS)
    expect(out).toEqual({
      kind: 'failed',
      reason: `it came back on v${OLD}; the update to v${NEW} did not install`,
      version: OLD,
    })
  })

  it('the job reports failed → failed with the daemon’s real reason', async () => {
    const reason = 'v0.46.3 did not start after 3 tries; the previous version was put back'
    const host = scriptedHost(
      [null, { phase: 'ready', version: OLD, instanceId: 'bbbb' }],
      [{ phase: 'failed', error: reason, target_version: NEW, from_version: OLD, running_version: OLD }],
    )
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({ kind: 'failed', reason, version: OLD })
  })

  it('an error state before any restart (bundled-app: the app reported failure) → failed with that reason', async () => {
    const host = scriptedHost(
      [{ phase: 'ready', version: OLD, instanceId: 'aaaa' }],
      [{ phase: 'failed', error: 'No update available for this app.' }],
    )
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({
      kind: 'failed',
      reason: 'No update available for this app.',
      version: OLD,
    })
  })

  it('the old daemon still answering ready (not restarted yet) keeps watching — no false rollback', async () => {
    const host = scriptedHost(
      [
        { phase: 'ready', version: OLD, instanceId: 'aaaa' },
        { phase: 'ready', version: OLD, instanceId: 'aaaa' },
        null,
        { phase: 'ready', version: NEW, instanceId: 'cccc' },
      ],
      [{ phase: 'downloading' }, { phase: 'downloading' }, null],
    )
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({ kind: 'success', version: NEW })
    expect(host.ticks()).toBe(4)
  })

  it('a network blip (same instanceId) while the job still downloads is not a restart', async () => {
    const host = scriptedHost(
      [
        null, // blip
        { phase: 'ready', version: OLD, instanceId: 'aaaa' }, // same daemon
        { phase: 'ready', version: NEW, instanceId: 'dddd' },
      ],
      [{ phase: 'downloading' }, null],
    )
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({ kind: 'success', version: NEW })
  })

  it('the Linux boot verifier still settling (job restarting) keeps watching instead of failing', async () => {
    const host = scriptedHost(
      [
        null,
        { phase: 'ready', version: OLD, instanceId: 'bbbb' },
        { phase: 'ready', version: NEW, instanceId: 'cccc' },
      ],
      [{ phase: 'restarting' }, { phase: 'done', running_version: NEW }],
    )
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({ kind: 'success', version: NEW })
  })

  it('the host never comes back → "couldn’t confirm", not "failed"', async () => {
    const host = scriptedHost([null])
    const out = await watchUpdateComeback(host.deps, OPTS)
    expect(out).toEqual({ kind: 'unconfirmed', reason: 'it did not come back within 10 minutes' })
    expect(host.slowCalls()).toBe(1)
    // 10 min / 2.5 s = 240 polls, then it stops.
    expect(host.ticks()).toBe(240)
  })

  it('still answering on the old version and never restarted by the deadline → "couldn’t confirm"', async () => {
    const host = scriptedHost([{ phase: 'ready', version: OLD, instanceId: 'aaaa' }], [{ phase: 'downloading' }])
    expect(await watchUpdateComeback(host.deps, OPTS)).toEqual({
      kind: 'unconfirmed',
      reason: `it is still answering on v${OLD} and has not restarted yet`,
      version: OLD,
    })
  })

  it('stops quietly (null) once the row unmounts', async () => {
    let alive = true
    const host = scriptedHost([null])
    const deps: UpdateComebackDeps = {
      ...host.deps,
      sleep: async (ms) => {
        await host.deps.sleep(ms)
        alive = false
      },
      alive: () => alive,
    }
    expect(await watchUpdateComeback(deps, OPTS)).toBeNull()
  })
})

describe('comeback copy', () => {
  it('names the host in every state', () => {
    expect(updateReconnectingCopy('akzm')).toBe('Updating akzm… reconnecting…')
    expect(updateSucceededCopy('akzm', NEW)).toBe(`Updated akzm to v${NEW}.`)
    expect(updateFailedCopy('akzm', `it came back on v${OLD}; the update to v${NEW} did not install`)).toBe(
      `Update failed on akzm: it came back on v${OLD}; the update to v${NEW} did not install.`,
    )
    expect(updateFailedCopy('akzm', 'No update available for this app.')).toBe(
      'Update failed on akzm: No update available for this app.',
    )
    expect(updateUnconfirmedCopy('akzm', 'it did not come back within 10 minutes')).toBe(
      "Couldn't confirm the update on akzm: it did not come back within 10 minutes. Check for updates to see its version.",
    )
  })
})
