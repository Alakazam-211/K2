// 0.46.7 Workbench: the renderer half of the Workbench rows. A write needs
// the person's press; start / land / settle wait for K2's sheet, and the
// widget never sees the sheet's id, only a closed answer.
import { describe, expect, it, vi } from 'vitest'
import { ZEN_WORK_VERBS, ZEN_WORK_WRITES, ZenWorkError, createZenCustomWork, zenWorkErrorFrom, type ZenWorkChange } from './zen-custom-work'

const ask = (kind: ZenWorkChange['kind'], secretId = 'sid-1') =>
  new Error(JSON.stringify({ error: { code: 'confirm_required', hint: 'K2 asks', change: { kind, name: 'fix' }, secretId } }))

function work(opts: { pressed?: boolean; yes?: boolean; answers?: Array<unknown> } = {}) {
  const calls: Array<{ verb: string; args: unknown[]; token?: string }> = []
  const answers = [...(opts.answers ?? [])]
  const confirm = vi.fn(async () => opts.yes ?? true)
  const w = createZenCustomWork({
    call: async (verb, args, token) => {
      calls.push({ verb, args, token })
      const next = answers.shift()
      if (next instanceof Error) throw next
      return next
    },
    pressed: () => opts.pressed ?? true,
    confirm,
  })
  return { w, calls, confirm }
}

describe('Workbench rows on the bridge', () => {
  it('lists the ten daemon rows, five of them writes', () => {
    expect([...ZEN_WORK_VERBS].sort()).toEqual(
      ['agents.settle', 'agents.start', 'agents.stop', 'compute.jobs', 'git.diffFile', 'git.land', 'git.log', 'git.note', 'git.snapshot', 'work.list'].sort(),
    )
    expect([...ZEN_WORK_WRITES].sort()).toEqual(['agents.settle', 'agents.start', 'agents.stop', 'git.land', 'git.note'])
    for (const v of ZEN_WORK_WRITES) expect(ZEN_WORK_VERBS.has(v)).toBe(true)
  })

  it('reads go straight through, without a press', async () => {
    const { w, calls } = work({ pressed: false, answers: [[{ address: 'k2/fix::local' }]] })
    await expect(w.call('work.list', [])).resolves.toEqual([{ address: 'k2/fix::local' }])
    expect(calls).toEqual([{ verb: 'work.list', args: [], token: undefined }])
  })

  it('a write without a press never reaches the daemon', async () => {
    const { w, calls } = work({ pressed: false })
    for (const verb of ZEN_WORK_WRITES) {
      const err = await w.call(verb, [{ address: 'k2/fix::local' }]).catch((e: unknown) => e)
      expect(err).toBeInstanceOf(ZenWorkError)
      expect((err as ZenWorkError).wire.code).toBe('failed')
      expect((err as ZenWorkError).message).toMatch(/needs a click/)
    }
    expect(calls).toEqual([])
  })

  it('Land asks K2’s sheet, then repeats the call with the daemon’s id', async () => {
    const landed = { landed: true, commits: 2, head: 'abc1234', conflict: [], cancelled: false }
    const { w, calls, confirm } = work({ answers: [ask('land'), landed] })
    await expect(w.call('git.land', [{ address: 'k2/fix::local' }])).resolves.toEqual(landed)
    expect(confirm).toHaveBeenCalledWith({ kind: 'land', name: 'fix' })
    expect(calls.map((c) => c.token)).toEqual([undefined, 'sid-1'])
  })

  it('Cancel answers a closed shape (GTC-5), and a cancelled Start is declined', async () => {
    let t = work({ yes: false, answers: [ask('land')] })
    await expect(t.w.call('git.land', [{ address: 'a/b::local' }])).resolves.toEqual({ landed: false, commits: 0, head: null, conflict: [], cancelled: true })
    expect(t.calls).toHaveLength(1)
    t = work({ yes: false, answers: [ask('settle')] })
    await expect(t.w.call('agents.settle', [{ address: 'a/b::local' }])).resolves.toEqual({ settled: false, discarded: false, cancelled: true })
    t = work({ yes: false, answers: [ask('start')] })
    const err = await t.w.call('agents.start', [{ workspace: 'a::local', name: 'b', brief: 'c' }]).catch((e: unknown) => e)
    expect((err as ZenWorkError).wire.code).toBe('declined')
  })

  it('one sheet at a time per frame', async () => {
    let release: (yes: boolean) => void = () => undefined
    const calls: string[] = []
    const w = createZenCustomWork({
      call: async (verb, _args, token) => {
        calls.push(`${verb}:${token ?? ''}`)
        if (!token) throw ask(verb === 'git.land' ? 'land' : 'settle')
        return { ok: true }
      },
      pressed: () => true,
      confirm: () => new Promise<boolean>((r) => (release = r)),
    })
    const first = w.call('git.land', [{ address: 'a/b::local' }])
    await new Promise((r) => setTimeout(r, 0))
    const second = await w.call('agents.settle', [{ address: 'a/b::local' }]).catch((e: unknown) => e)
    expect((second as ZenWorkError).wire.code).toBe('rate_limited')
    release(true)
    await expect(first).resolves.toEqual({ ok: true })
  })

  it('reads the daemon’s refusals into the shared codes; an unknown code is failed', () => {
    expect(zenWorkErrorFrom(new Error(JSON.stringify({ error: { code: 'not_bound', hint: 'nope' } }))).wire).toEqual({ code: 'not_bound', message: 'nope' })
    expect(zenWorkErrorFrom(new Error(JSON.stringify({ error: { code: 'weird' } }))).wire.code).toBe('failed')
    expect(zenWorkErrorFrom(new Error('plain text')).wire).toEqual({ code: 'failed', message: 'plain text' })
    // A confirm_required without a known kind or id is not a sheet.
    expect(zenWorkErrorFrom(new Error(JSON.stringify({ error: { code: 'confirm_required', change: { kind: 'other' }, secretId: 'x' } }))).ask).toBeNull()
  })

  it('an unknown verb is refused here', async () => {
    const { w } = work()
    const err = await w.call('store.get', ['x']).catch((e: unknown) => e)
    expect((err as ZenWorkError).wire.code).toBe('unknown_verb')
  })
})
