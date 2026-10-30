// Settings → Chat Hosting (Sew S32 K27): the wire helpers and the tables
// the page renders. The alarm and step tables are checked against the
// daemon's own Rust sources, so a new alarm or step there fails here.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import {
  SEW_ALARMS,
  SEW_ENABLE_STEPS,
  alarmInfo,
  chatGrantsOf,
  confirmSecretId,
  downloadNote,
  enableBody,
  enableLabel,
  isNotBuilt,
  ownerHandoffUrl,
  resetSewStatusCacheForTests,
  sewErrorInfo,
  sewErrorMessage,
  sewStatusCached,
} from './sew-api'

const here = dirname(fileURLToPath(import.meta.url))
const sewSrc = resolve(here, '../../../../../crates/k2-daemon/src/sew')
const rust = (f: string): string => readFileSync(resolve(sewSrc, f), 'utf8')

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  resetSewStatusCacheForTests()
})

describe('errors', () => {
  it('reads code, status and hint from the thrown DaemonHttpError', () => {
    const e = new DaemonHttpError('Sew is not installed', 409, 'not_installed')
    expect(sewErrorInfo(e)).toEqual({ code: 'not_installed', status: 409, message: 'Sew is not installed' })
    expect(sewErrorMessage(e)).toBe('Sew is not installed')
  })

  it('owner_only and a bare 403 read as the no-permission line', () => {
    expect(sewErrorMessage(new DaemonHttpError('x', 403, 'owner_only'))).toBe('Only the box owner or an admin can manage Chat / Voice Hosting.')
    expect(sewErrorMessage(new DaemonHttpError('x', 403, null))).toBe('Only the box owner or an admin can manage Chat / Voice Hosting.')
    expect(sewErrorMessage(new DaemonHttpError('Another Sew action', 409, 'lifecycle_in_progress'))).toBe(
      'Another Chat / Voice Hosting action is running. Wait for it to finish.',
    )
    expect(sewErrorInfo('plain')).toEqual({ code: null, status: null, message: 'plain' })
  })

  it('isNotBuilt on 404, 501 and not_built only', () => {
    expect(isNotBuilt(new DaemonHttpError('no', 404, null))).toBe(true)
    expect(isNotBuilt(new DaemonHttpError('no', 501, null))).toBe(true)
    expect(isNotBuilt(new DaemonHttpError('no', 400, 'not_built'))).toBe(true)
    expect(isNotBuilt(new DaemonHttpError('no', 403, 'forbidden'))).toBe(false)
    expect(isNotBuilt(new Error('network'))).toBe(false)
  })
})

describe('tables match the daemon', () => {
  it('every alarm the supervisor and voice push has a title, fix and severity', () => {
    const pushed = new Set<string>()
    for (const f of ['supervisor.rs', 'voice.rs']) {
      for (const m of rust(f).matchAll(/\b(?:a|alarms)\.push\(\s*"([a-z0-9_]+)"/g)) pushed.add(m[1])
    }
    const backup = /BACKUP_ALARMS: &\[&str\] = &\[([^\]]+)\]/.exec(rust('sew_wire.rs'))
    if (!backup) throw new Error('BACKUP_ALARMS not found in sew_wire.rs')
    for (const m of backup[1].matchAll(/"([a-z_]+)"/g)) pushed.add(m[1])
    expect(pushed.size).toBeGreaterThan(10)
    const missing = [...pushed].filter((a) => !SEW_ALARMS[a])
    expect(missing).toEqual([])
    for (const [name, info] of Object.entries(SEW_ALARMS)) {
      expect(info.title.length, name).toBeGreaterThan(3)
      expect(info.fix.length, name).toBeGreaterThan(3)
      expect(['error', 'warn']).toContain(info.severity)
    }
  })

  it('an unknown alarm renders verbatim with the status hint', () => {
    expect(alarmInfo('brand_new')).toEqual({ severity: 'warn', title: 'brand_new', fix: 'Run k2 hostchat status for details.' })
  })

  it('the enable steps are the enable machine’s, in order', () => {
    const src = rust('supervisor.rs')
    const start = src.indexOf('fn enable_steps(')
    const end = src.indexOf('\n}\n', start)
    if (start < 0 || end < 0) throw new Error('enable_steps not found')
    const ids: string[] = []
    for (const m of src.slice(start, end).matchAll(/\bstep\(env, "([a-z-]+)"\)/g)) if (!ids.includes(m[1])) ids.push(m[1])
    expect(SEW_ENABLE_STEPS.map((s) => s.id)).toEqual(ids)
  })

  it('the wizard label per state', () => {
    expect(enableLabel('not-installed')).toBe('Enable Chat / Voice Hosting')
    expect(enableLabel('error')).toBe('Resume')
    for (const s of ['disabled', 'stopped', 'removed']) expect(enableLabel(s)).toBe('Re-enable')
    for (const s of ['running', 'degraded', 'installing', 'upgrading']) expect(enableLabel(s)).toBeNull()
  })
})

describe('bodies', () => {
  it('enable sends identity always, voice and paths only when set', () => {
    expect(enableBody({ identity: 'k2' })).toEqual({ identity: 'k2' })
    expect(enableBody({ identity: 'standalone', tarballPath: '  ', recoveryKeyFile: '' })).toEqual({ identity: 'standalone' })
    expect(
      enableBody({ identity: 'k2', tarballPath: ' /t/sew.tar.gz ', voice: { publicUrl: 'wss://v.box.k2.dev', nodeIp: '', mediaTarballPath: ' ' } }),
    ).toEqual({ identity: 'k2', tarballPath: '/t/sew.tar.gz', voice: { publicUrl: 'wss://v.box.k2.dev' } })
    expect(enableBody({ identity: 'k2', voice: {} })).toEqual({ identity: 'k2', voice: {} })
  })

  it('the owner handoff URL carries the code in the fragment, never the query', () => {
    const u = ownerHandoffUrl('https://hearth.box.k2.dev/', 'k2sho_abc-_')
    expect(u).toBe('https://hearth.box.k2.dev/k2/owner-handoff#code=k2sho_abc-_')
    expect(u).not.toContain('?')
  })

  it('confirm secret ids: install id or none; the handoff is unbound', () => {
    expect(confirmSecretId('sew-recovery-key', 'ins_1')).toBe('ins_1')
    expect(confirmSecretId('sew-purge', null)).toBe('none')
    expect(confirmSecretId('chat-owner-handoff', 'ins_1')).toBeUndefined()
  })

  it('chat grants: only kind chat, hook state kept', () => {
    expect(
      chatGrantsOf({
        grants: [
          { id: 'g1', kind: 'chat', subjectKind: 'principal', subjectId: 'p1', scope: 'guest', enabled: true, hookState: 'blocked', hookError: 'sew down' },
          { id: 'g2', kind: 'app', subjectKind: 'principal', subjectId: 'p1', scope: null, enabled: true },
          { id: 'g3', kind: 'chat', subjectKind: 'connect', subjectId: 'c1', scope: null, enabled: false },
        ],
      }),
    ).toEqual([
      { id: 'g1', subjectKind: 'principal', subjectId: 'p1', scope: 'guest', enabled: true, hookState: 'blocked', hookError: 'sew down' },
      { id: 'g3', subjectKind: 'connect', subjectId: 'c1', scope: null, enabled: false },
    ])
  })
})

describe('download note (0.46.7)', () => {
  it('reads the daemon progress.download the way the supervisor writes it', () => {
    // The keys the daemon writes (supervisor.rs download_note / obtain_release).
    const src = rust('supervisor.rs')
    for (const k of ['"file"', '"label"', '"url"', '"bytes"', '"total"', '"source"']) expect(src).toContain(k)
    expect(downloadNote({ label: 'Sew 0.5.0', source: 'download', bytes: 12_300_000, total: 24_667_386 })).toBe('Sew 0.5.0: 12.3 of 24.7 MB')
    expect(downloadNote({ label: 'Sew 0.5.0', source: 'download', bytes: 1_000_000, total: null })).toBe('Sew 0.5.0: 1.0 MB')
    expect(downloadNote({ label: 'Sew 0.5.0', source: 'cache' })).toBe("Sew 0.5.0 from K2's download cache")
    expect(downloadNote({ label: 'Sew 0.5.0', source: 'tarball' })).toBe('Sew 0.5.0 from the file you named')
    expect(downloadNote(null)).toBeNull()
  })
})

describe('shared status read', () => {
  it('one read within the TTL, however many callers', async () => {
    h.daemonCliGet.mockResolvedValue({ supported: true, state: 'running' })
    const [a, b] = await Promise.all([sewStatusCached(), sewStatusCached()])
    await sewStatusCached()
    expect(a).toBe(b)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    expect(h.daemonCliGet).toHaveBeenCalledWith('sew/status', undefined)
  })

  it('a failed read is not cached', async () => {
    h.daemonCliGet.mockRejectedValueOnce(new Error('down')).mockResolvedValueOnce({ supported: true, state: 'running' })
    await expect(sewStatusCached()).rejects.toThrow('down')
    await expect(sewStatusCached()).resolves.toEqual({ supported: true, state: 'running' })
    expect(h.daemonCliGet).toHaveBeenCalledTimes(2)
  })
})
