// Reading a remote host's update replies + driving a Connections update job
// (z3.k2.dev, 2026-11-05). Rosson's client showed "vundefined" and the
// update sat `staged` on the host with no apply: replies that weren't the
// route's answer were cast to the result type, and a status without a phase
// (or a tile that unmounted) left the job with nothing to apply it.

import { describe, it, expect, vi } from 'vitest'
import {
  readUpdateCheck,
  readUpdateStatus,
  readUpdateStart,
  replySnippet,
  UpdateReplyError,
  driveUpdateJob,
  keepUpdateJob,
  keptUpdateJob,
  forgetUpdateJob,
  type UpdateCheckResult,
  type UpdateJobDeps,
} from './update-host'

const CHECK: UpdateCheckResult = {
  current: '0.46.7',
  latest: '0.46.8',
  available: true,
  installKind: 'standalone',
}

function thrown(fn: () => unknown): Error {
  try {
    fn()
  } catch (e) {
    if (e instanceof Error) return e
    throw new Error(`threw a non-Error: ${String(e)}`)
  }
  throw new Error('did not throw')
}

describe('readUpdateCheck', () => {
  it('passes a real check through unchanged', () => {
    expect(readUpdateCheck('z3', CHECK)).toBe(CHECK)
  })

  it.each([
    ['an HTML page (non-JSON 200, as daemon-cli hands it back)', '<!doctype html><title>Sign in</title>'],
    ['an empty body', undefined],
    ['another route’s JSON', { ok: true, restarting: true }],
    ['a check with no current', { latest: '0.46.8', available: true }],
    ['an empty current', { current: '', latest: '0.46.8', available: false }],
    ['available as a string', { current: '0.46.7', latest: '0.46.8', available: 'true' }],
    ['an array', [CHECK]],
  ])('%s is a named UpdateReplyError', (_what, body) => {
    const err = thrown(() => readUpdateCheck('z3', body))
    expect(err).toBeInstanceOf(UpdateReplyError)
    expect(err.name).toBe('UpdateReplyError')
    expect(err.message).toMatch(/^Unexpected reply from z3 to the update check \(expected its current and latest version\): got /)
    expect(err.message).not.toContain('vundefined')
  })

  it('quotes what came back, one line, cut at 80 characters', () => {
    const err = thrown(() => readUpdateCheck('z3', `<html>\n  <body>${'x'.repeat(200)}</body>\n</html>`))
    expect(err.message).toContain('got "<html> <body>xxx')
    expect(err.message.endsWith('…"')).toBe(true)
  })
})

describe('readUpdateStatus', () => {
  it('passes every daemon phase', () => {
    for (const phase of ['downloading', 'verifying', 'staged', 'applying', 'restarting', 'done', 'failed', 'rolled-back']) {
      expect(readUpdateStatus('z3', { phase, progress: 1 }).phase).toBe(phase)
    }
  })

  it.each([
    ['an HTML page', '<html>gateway</html>'],
    ['no phase', { progress: 1, error: null }],
    ['an unknown phase', { phase: 'installing' }],
    ['an empty body', ''],
  ])('%s is a named UpdateReplyError', (_what, body) => {
    const err = thrown(() => readUpdateStatus('z3', body))
    expect(err).toBeInstanceOf(UpdateReplyError)
    expect(err.message).toMatch(/^Unexpected reply from z3 to the update status \(expected a phase\): got /)
  })
})

describe('readUpdateStart', () => {
  it('returns the job id', () => {
    expect(readUpdateStart('z3', { job_id: 'job-1' })).toEqual({ job_id: 'job-1' })
  })

  it('a reply without a job id is a named UpdateReplyError', () => {
    const err = thrown(() => readUpdateStart('z3', { ok: true }))
    expect(err).toBeInstanceOf(UpdateReplyError)
    expect(err.message).toBe('Unexpected reply from z3 to the update start (expected a job id): got "{"ok":true}"')
  })
})

describe('replySnippet', () => {
  it('names an empty reply', () => {
    expect(replySnippet(undefined)).toBe('an empty reply')
    expect(replySnippet('')).toBe('an empty reply')
    expect(replySnippet(null)).toBe('an empty reply')
  })
})

describe('kept update jobs (General row survives leaving the page)', () => {
  it('keeps, returns a copy, and forgets per host', () => {
    keepUpdateJob('h1', { jobId: 'job-1', check: CHECK, applySent: false })
    const kept = keptUpdateJob('h1')
    expect(kept).toEqual({ jobId: 'job-1', check: CHECK, applySent: false })
    if (!kept) throw new Error('job not kept')
    kept.applySent = true
    expect(keptUpdateJob('h1')?.applySent).toBe(false)
    expect(keptUpdateJob('h2')).toBeNull()
    forgetUpdateJob('h1')
    expect(keptUpdateJob('h1')).toBeNull()
  })
})

/** A scripted host: `replies` answers each status poll in turn (an Error is
 *  thrown); the last one repeats. */
function job(replies: unknown[], opts: Partial<UpdateJobDeps> = {}) {
  let i = 0
  const status = vi.fn(async () => {
    const r = replies[Math.min(i, replies.length - 1)]
    i += 1
    if (r instanceof Error) throw r
    return r
  })
  const apply = vi.fn(async () => ({ ok: true, applying: true }))
  const deps: UpdateJobDeps = {
    hostLabel: 'z3',
    installKind: 'standalone',
    status,
    apply,
    sleep: async () => {},
    maxPolls: 5,
    ...opts,
  }
  return { deps, status, apply }
}

describe('driveUpdateJob (Settings → Connections update)', () => {
  it('standalone: staged → apply once with the job → restarting = applied', async () => {
    const { deps, apply } = job([{ phase: 'downloading', progress: 0.5 }, { phase: 'staged' }, { phase: 'restarting' }])
    await expect(driveUpdateJob(deps)).resolves.toEqual({ kind: 'applied' })
    expect(apply).toHaveBeenCalledTimes(1)
  })

  it('a status reply without a phase fails at once, before apply — no 3-minute wait', async () => {
    const { deps, status, apply } = job(['<html>gateway</html>'], { maxPolls: 90 })
    const out = await driveUpdateJob(deps)
    expect(out.kind).toBe('poll-failed')
    if (out.kind !== 'poll-failed') throw new Error(out.kind)
    expect(out.err).toBeInstanceOf(UpdateReplyError)
    expect((out.err as Error).message).toContain('Unexpected reply from z3 to the update status')
    expect(status).toHaveBeenCalledTimes(1)
    expect(apply).not.toHaveBeenCalled()
  })

  it('an unreadable reply AFTER apply is the host going away: watch it come back', async () => {
    const { deps, apply } = job([{ phase: 'staged' }, '<html>gateway</html>'])
    await expect(driveUpdateJob(deps)).resolves.toEqual({ kind: 'unreachable' })
    expect(apply).toHaveBeenCalledTimes(1)
  })

  it('a dropped connection after apply is the install (unreachable), before it a failure', async () => {
    const after = job([{ phase: 'staged' }, new TypeError('Load failed')])
    await expect(driveUpdateJob(after.deps)).resolves.toEqual({ kind: 'unreachable' })

    const before = job([new TypeError('Load failed')])
    const out = await driveUpdateJob(before.deps)
    expect(out.kind).toBe('poll-failed')
    expect(before.apply).not.toHaveBeenCalled()
  })

  it('apply refused by a live host is apply-failed; a dropped apply is unreachable', async () => {
    const refused = job([{ phase: 'staged' }])
    refused.apply.mockRejectedValueOnce(Object.assign(new Error('swap binary: EACCES'), { status: 500 }))
    const out = await driveUpdateJob(refused.deps)
    expect(out.kind).toBe('apply-failed')

    const dropped = job([{ phase: 'staged' }])
    dropped.apply.mockRejectedValueOnce(new TypeError('Load failed'))
    await expect(driveUpdateJob(dropped.deps)).resolves.toEqual({ kind: 'unreachable' })
  })

  it('a failed job reports its status', async () => {
    const { deps } = job([{ phase: 'failed', error: 'signature mismatch' }])
    await expect(driveUpdateJob(deps)).resolves.toEqual({
      kind: 'job-failed',
      status: { phase: 'failed', error: 'signature mismatch' },
    })
  })

  it('bundled-app: never applies; a drop after start is the app installing', async () => {
    const { deps, apply } = job([{ phase: 'downloading' }, new TypeError('Load failed')], { installKind: 'bundled-app' })
    await expect(driveUpdateJob(deps)).resolves.toEqual({ kind: 'unreachable' })
    expect(apply).not.toHaveBeenCalled()
  })

  it('a job that never reaches a verdict times out', async () => {
    const { deps } = job([{ phase: 'downloading' }], { maxPolls: 3 })
    await expect(driveUpdateJob(deps)).resolves.toEqual({ kind: 'timed-out' })
  })
})
