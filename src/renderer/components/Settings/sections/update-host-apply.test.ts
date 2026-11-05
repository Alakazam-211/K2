// Install & restart on a remote host: an `apply` the host turned down must
// say so at once (Rosson 2026-11-05: "when I click Install and restart
// after the download is done … it doesn't actually perform the update").
// A 400 "unknown job_id" / "not 'staged'" used to count as the install
// going ahead, so the row waited ten minutes for a restart that never came.
import { describe, expect, it } from 'vitest'
import {
  isAmbiguousApplyError,
  isJobStateRefusal,
  settleApplyRefusal,
  type UpdateStatusResult,
} from './update-host'

function http(message: string, status: number): Error {
  return Object.assign(new Error(message), { status })
}

const UNKNOWN = http('unknown job_id job-1', 400)
const NOT_STAGED = http("job job-1 is 'downloading', not 'staged' — cannot apply", 400)

function deps(over: {
  boot?: { instanceId?: string } | null
  job?: UpdateStatusResult | 'unknown' | null
  start?: string | undefined
  err?: unknown
}) {
  let jobReads = 0
  return {
    reads: () => jobReads,
    d: {
      hostLabel: 'z3',
      err: over.err ?? UNKNOWN,
      startInstanceId: 'start' in over ? over.start : 'inst-old',
      bootStatus: async () => ('boot' in over ? (over.boot ?? null) : { instanceId: 'inst-old' }),
      jobStatus: async () => {
        jobReads += 1
        return 'job' in over ? (over.job ?? null) : 'unknown'
      },
    },
  }
}

describe('isJobStateRefusal', () => {
  it('matches the daemon prepare_apply 400s, nothing else', () => {
    expect(isJobStateRefusal(UNKNOWN)).toBe(true)
    expect(isJobStateRefusal(NOT_STAGED)).toBe(true)
    expect(isJobStateRefusal(http('swap binary: Permission denied', 500))).toBe(false)
    expect(isJobStateRefusal(http('invalid or missing token', 403))).toBe(false)
    expect(isJobStateRefusal(new TypeError('Load failed'))).toBe(false)
    // Still "ambiguous" for the Connections loop's older rule.
    expect(isAmbiguousApplyError(UNKNOWN)).toBe(true)
  })
})

describe('settleApplyRefusal', () => {
  it('same daemon, job gone: refused, the download is gone (not a 10-minute wait)', async () => {
    const { d } = deps({})
    await expect(settleApplyRefusal(d)).resolves.toEqual({
      kind: 'refused',
      jobGone: true,
      message:
        'z3 no longer has the downloaded update (it restarted), so nothing was installed. Check for updates to download it again.',
    })
  })

  it('same daemon, job still staged or downloading: refused with the host reason', async () => {
    const { d } = deps({ err: NOT_STAGED, job: { phase: 'downloading' } })
    await expect(settleApplyRefusal(d)).resolves.toEqual({
      kind: 'refused',
      jobGone: false,
      message: "z3 did not install the update: job job-1 is 'downloading', not 'staged' — cannot apply",
    })
  })

  it('same daemon, job unreadable: refused (the same instance proves no restart)', async () => {
    const { d } = deps({ job: null })
    const v = await settleApplyRefusal(d)
    expect(v.kind).toBe('refused')
  })

  it('a retried apply that reached a NEW daemon: went ahead, the job is not even read', async () => {
    const { d, reads } = deps({ boot: { instanceId: 'inst-new' } })
    await expect(settleApplyRefusal(d)).resolves.toEqual({ kind: 'went-ahead' })
    expect(reads()).toBe(0)
  })

  it('the host is down: went ahead (it is installing)', async () => {
    const { d } = deps({ boot: null })
    await expect(settleApplyRefusal(d)).resolves.toEqual({ kind: 'went-ahead' })
  })

  it('the job moved on (applying / restarting / done / failed): went ahead, the watcher reports it', async () => {
    for (const phase of ['applying', 'restarting', 'done', 'failed', 'rolled-back'] as const) {
      const { d } = deps({ err: NOT_STAGED, job: { phase } })
      await expect(settleApplyRefusal(d)).resolves.toEqual({ kind: 'went-ahead' })
    }
  })

  it('no instance ids to compare and the job is gone: watched as before (cannot prove)', async () => {
    const { d } = deps({ start: undefined })
    await expect(settleApplyRefusal(d)).resolves.toEqual({ kind: 'went-ahead' })
    const { d: d2 } = deps({ boot: {} })
    await expect(settleApplyRefusal(d2)).resolves.toEqual({ kind: 'went-ahead' })
  })
})
