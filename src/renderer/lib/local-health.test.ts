// 0.46.18 — the mounted window's LOCAL server busy or gone (lib/local-health).

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  LOCAL_PROBE_TIMEOUTS_MS,
  LOCAL_TROUBLE_SURFACE_MS,
  localHealthLoopRunningForTests,
  localHealthLoopsStartedForTests,
  localHealthProbesInFlightForTests,
  localHealthStore,
  localHealthText,
  localProbeTimeoutMs,
  noteLocalOk,
  noteLocalTrouble,
  resetLocalHealthForTests,
  startLocalHealthWatch,
  type LocalProbeOutcome,
} from './local-health'

/** A scripted probe: each call parks until the test answers it. */
function scriptedProbe() {
  const calls: Array<{ timeoutMs: number; answer: (o: LocalProbeOutcome) => void }> = []
  return {
    calls,
    probe: (timeoutMs: number) =>
      new Promise<LocalProbeOutcome>((resolve) => {
        calls.push({ timeoutMs, answer: resolve })
      }),
  }
}

/** Let the loop's awaits run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}

let clock = 0
const sleeps: number[] = []
const deps = {
  now: () => clock,
  sleep: async (ms: number) => {
    sleeps.push(ms)
  },
  jitter: (ms: number) => ms,
}

beforeEach(() => {
  resetLocalHealthForTests()
  clock = 1_000_000
  sleeps.length = 0
})

afterEach(() => {
  resetLocalHealthForTests()
})

describe('local health (0.46.18)', () => {
  it('a storm of trouble runs ONE probe loop, never several', async () => {
    const p = scriptedProbe()
    startLocalHealthWatch({ ...deps, probe: p.probe })
    for (let i = 0; i < 25; i++) noteLocalTrouble()
    await flush()
    expect(localHealthLoopsStartedForTests()).toBe(1)
    expect(p.calls).toHaveLength(1)
    expect(localHealthProbesInFlightForTests()).toBe(1)
    // More trouble while a probe is out still adds nothing.
    noteLocalTrouble()
    noteLocalTrouble()
    await flush()
    expect(p.calls).toHaveLength(1)
    p.calls[0]!.answer('ok')
    await flush()
    expect(localHealthLoopRunningForTests()).toBe(false)
    expect(localHealthProbesInFlightForTests()).toBe(0)
  })

  it('a slow server gets longer each probe and the light busy note, then clears', async () => {
    const p = scriptedProbe()
    startLocalHealthWatch({ ...deps, probe: p.probe })
    noteLocalTrouble()
    await flush()
    expect(p.calls[0]!.timeoutMs).toBe(LOCAL_PROBE_TIMEOUTS_MS[0])

    clock += 2_000 // the first probe timed out
    p.calls[0]!.answer('slow')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('busy')
    expect(localHealthText(localHealthStore.getState().health)).toBe(
      'This computer’s K2 server is busy. Reconnecting…',
    )
    expect(p.calls[1]!.timeoutMs).toBe(LOCAL_PROBE_TIMEOUTS_MS[1])

    clock += 4_000
    p.calls[1]!.answer('slow')
    await flush()
    expect(p.calls[2]!.timeoutMs).toBe(LOCAL_PROBE_TIMEOUTS_MS[2])
    // It backs off between probes.
    expect(sleeps.slice(0, 2)).toEqual([500, 1_000])

    p.calls[2]!.answer('ok')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('ok')
    expect(localHealthLoopRunningForTests()).toBe(false)
  })

  it('trouble that clears inside the surface window never shows the note', async () => {
    const p = scriptedProbe()
    startLocalHealthWatch({ ...deps, probe: p.probe })
    noteLocalTrouble()
    await flush()
    clock += LOCAL_TROUBLE_SURFACE_MS - 1
    p.calls[0]!.answer('refused')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('ok')
    p.calls[1]!.answer('ok')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('ok')
  })

  it('a refused connection says can’t reach, still without blocking anything', async () => {
    const p = scriptedProbe()
    startLocalHealthWatch({ ...deps, probe: p.probe })
    noteLocalTrouble()
    await flush()
    clock += LOCAL_TROUBLE_SURFACE_MS
    p.calls[0]!.answer('refused')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('unreachable')
    expect(localHealthText(localHealthStore.getState().health)).toBe(
      'Can’t reach this computer’s K2 server. Reconnecting…',
    )
  })

  it('a socket that opens ends the loop at once and drops a late probe answer', async () => {
    const p = scriptedProbe()
    startLocalHealthWatch({ ...deps, probe: p.probe })
    noteLocalTrouble()
    await flush()
    clock += 2_000
    p.calls[0]!.answer('slow')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('busy')
    noteLocalOk()
    expect(localHealthStore.getState().health.kind).toBe('ok')
    expect(localHealthLoopRunningForTests()).toBe(false)
    // The probe that was out answers late: ignored.
    p.calls[1]!.answer('slow')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('ok')
    expect(p.calls).toHaveLength(2)
  })

  it('without a watch (a remote window, or before mount) trouble does nothing', async () => {
    noteLocalTrouble()
    await flush()
    expect(localHealthLoopsStartedForTests()).toBe(0)
    expect(localHealthStore.getState().health.kind).toBe('ok')
  })

  it('stopping the watch clears the note', async () => {
    const p = scriptedProbe()
    const stop = startLocalHealthWatch({ ...deps, probe: p.probe })
    noteLocalTrouble()
    await flush()
    clock += 2_000
    p.calls[0]!.answer('slow')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('busy')
    stop()
    expect(localHealthStore.getState().health.kind).toBe('ok')
    p.calls[1]!.answer('slow')
    await flush()
    expect(localHealthStore.getState().health.kind).toBe('ok')
  })

  it('the probe budget grows to a 15 s cap', () => {
    expect([0, 1, 2, 3, 4, 9].map(localProbeTimeoutMs)).toEqual([2_000, 4_000, 8_000, 15_000, 15_000, 15_000])
  })
})
