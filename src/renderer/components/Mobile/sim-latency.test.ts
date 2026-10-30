// The latency readout's maths (sim-latency), fail loud: input → the first
// frame past the one in flight, the median of the last N, fps.

import { describe, expect, it } from 'vitest'
import { LatencyMeter, latencyText, median } from './sim-latency'

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
