// The latency readout's maths (sim-latency), fail loud: input → the first
// frame past the one in flight, the median of the last N, fps.

import { describe, expect, it } from 'vitest'
import { detailText, FrameStageClock, LatencyMeter, latencyText, median, percentile, PhotonProbe, pixelChanged } from './sim-latency'

describe('latency readout', () => {
  it('median of odd and even counts; none before a sample', () => {
    expect(median([])).toBeNull()
    expect(median([30, 10, 20])).toBe(20)
    expect(median([40, 10, 30, 20])).toBe(25)
  })

  it('a still screen: the next frame is the input’s', () => {
    const m = new LatencyMeter()
    m.frame(1, 0)
    m.input(500)
    m.frame(2, 548)
    expect(m.readout(548)).toMatchObject({ medianMs: 48, samples: 1 })
  })

  it('frames already flowing: the next frame may predate the input, so it waits one more', () => {
    const m = new LatencyMeter()
    m.frame(1, 0)
    m.frame(2, 33)
    m.frame(3, 66)
    m.input(70)
    m.frame(4, 99)
    expect(m.readout(99).samples).toBe(0)
    m.frame(5, 132)
    expect(m.readout(132)).toMatchObject({ medianMs: 62, samples: 1 })
  })

  it('one probe at a time; an input that changed nothing times out', () => {
    const m = new LatencyMeter()
    m.input(0)
    m.input(10) // ignored: a probe is open
    m.frame(1, 40)
    expect(m.readout(40)).toMatchObject({ medianMs: 40, samples: 1 })
    m.input(1000)
    m.frame(2, 4000) // 3 s later: not this input's frame
    expect(m.readout(4000).samples).toBe(1)
  })

  it('keeps the last N samples', () => {
    const m = new LatencyMeter(3)
    let t = 0
    let seq = 0
    for (const ms of [100, 100, 10, 20, 30]) {
      t += 1000
      m.input(t)
      m.frame(++seq, t + ms)
    }
    expect(m.readout(t + 30)).toMatchObject({ medianMs: 20, samples: 3 })
  })

  it('fps counts the frames drawn in the last second', () => {
    const m = new LatencyMeter()
    for (let i = 0; i < 45; i++) m.frame(i + 1, i * 33.4)
    // Frames at 503.6 … 1469.6 ms are within a second of 1470.
    expect(m.readout(1470).fps).toBe(30)
    expect(m.readout(5000).fps).toBe(0)
  })

  it('the HUD line', () => {
    expect(latencyText({ medianMs: null, samples: 0, fps: 30 })).toBe('input → frame — · 30 fps')
    expect(latencyText({ medianMs: 48, samples: 12, fps: 29 })).toBe('input → frame 48 ms (median of 12) · 29 fps')
  })
})

describe('M0: photon probe, client stages, the HUD detail line', () => {
  it('tap → photon: the first frame whose centre pixel changed; a still frame is not it', () => {
    const p = new PhotonProbe()
    p.frame(10, [0, 0, 0])
    expect(p.readout().samples).toBe(0)
    p.arm(100, [20, 20, 20])
    p.frame(130, [30, 25, 20]) // JPEG noise, not the change
    expect(p.waiting).toBe(true)
    p.frame(162, [250, 20, 20])
    expect(p.waiting).toBe(false)
    expect(p.readout()).toEqual({ p50: 62, p95: 62, samples: 1 })
  })

  it('no frame on screen at the tap: nothing to compare, no probe; an input that changes nothing times out', () => {
    const p = new PhotonProbe()
    p.arm(0, null)
    expect(p.waiting).toBe(false)
    p.arm(0, [0, 0, 0])
    p.frame(2500, [255, 255, 255])
    expect(p.readout().samples).toBe(0)
    expect(p.waiting).toBe(false)
  })

  it('p50 / p95 are nearest-rank', () => {
    const xs = Array.from({ length: 100 }, (_, i) => i + 1)
    expect(percentile(xs, 0.5)).toBe(50)
    expect(percentile(xs, 0.95)).toBe(95)
    expect(percentile([], 0.5)).toBeNull()
    expect(pixelChanged([0, 0, 0], [0, 0, 49])).toBe(true)
    expect(pixelChanged([0, 0, 0], [48, 48, 48])).toBe(false)
  })

  it('client stages: socket → decoded → drawn per seq; undrawn frames are dropped', () => {
    const c = new FrameStageClock()
    expect(c.readout()).toEqual({ decodeMs: null, drawMs: null })
    c.mark(1, 'recv', 0)
    c.mark(1, 'decoded', 4)
    c.mark(1, 'drawn', 5)
    c.mark(2, 'recv', 10) // stale: never drawn
    for (let s = 3; s < 60; s++) c.mark(s, 'recv', s)
    expect(c.readout()).toEqual({ decodeMs: 4, drawMs: 1 })
  })

  it('the detail line shows only what has data', () => {
    const none = { p50: null, p95: null, samples: 0 }
    expect(detailText(none, { decodeMs: null, drawMs: null }, null)).toBeNull()
    const line = detailText(
      { p50: 61, p95: 97, samples: 20 },
      { decodeMs: 3.2, drawMs: 0.4 },
      {
        frames: 60,
        stages: [{ stage: 'total', n: 60, p50: 12.5, p95: 20, max: 31 }],
        inputs: [{ id: 1, recvMs: 5, laneMs: 2 }],
        hubCpuMs: 41.2,
        encoderCpuMs: 120,
      },
    )
    expect(line).toBe('tap → photon 61 / 97 ms (p50 / p95 of 20) · daemon capture → sent 12.5 ms · input 2.0 ms · hub 41 · enc 120 ms CPU/s · decode 3.2 · draw 0.4 ms')
  })
})
