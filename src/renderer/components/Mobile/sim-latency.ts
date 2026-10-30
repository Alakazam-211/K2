// The Mobile tab's latency readout (0.46.7; off by default, for measuring):
// the time from a pointer input to the first frame drawn after it, and the
// frames drawn per second. Pure; the pane feeds it.
//
// The wire has no timestamp echo, so "the frame that reflects the input" is
// the first frame whose seq is past the one in flight when the input went:
//  - the screen was still (no frame for STILL_MS): the next frame is the
//    input's (the daemon sends nothing while the screen doesn't change);
//  - frames were already flowing: skip one more, since the next may have
//    been captured before the input landed.
// An input whose frame never comes (it changed nothing) is dropped after
// TIMEOUT_MS. One probe at a time; the readout is the median of the last N.

export const LATENCY_WINDOW = 20
export const STILL_MS = 120
export const TIMEOUT_MS = 2000
export const FPS_WINDOW_MS = 1000

export interface LatencyReadout {
  /** Median input → frame of the last `samples` probes; null before one. */
  medianMs: number | null
  samples: number
  /** Frames drawn in the last second. */
  fps: number
}

export function median(xs: readonly number[]): number | null {
  if (xs.length === 0) return null
  const s = [...xs].sort((a, b) => a - b)
  const mid = s.length >> 1
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2
}

export class LatencyMeter {
  private probe: { at: number; afterSeq: number } | null = null
  private lastSeq = 0
  private lastFrameAt = -Infinity
  private readonly frames: number[] = []
  private readonly samples: number[] = []

  constructor(private readonly window = LATENCY_WINDOW) {}

  /** An input went to the device at `now` (ms). */
  input(now: number): void {
    if (this.probe && now - this.probe.at <= TIMEOUT_MS) return
    const still = now - this.lastFrameAt >= STILL_MS
    this.probe = { at: now, afterSeq: this.lastSeq + (still ? 0 : 1) }
  }

  /** Frame `seq` was drawn at `now` (ms). */
  frame(seq: number, now: number): void {
    this.lastSeq = seq
    this.lastFrameAt = now
    this.frames.push(now)
    while (this.frames.length && now - this.frames[0] > FPS_WINDOW_MS) this.frames.shift()
    const p = this.probe
    if (!p) return
    if (now - p.at > TIMEOUT_MS) {
      this.probe = null
      return
    }
    if (seq <= p.afterSeq) return
    this.probe = null
    this.samples.push(now - p.at)
    if (this.samples.length > this.window) this.samples.shift()
  }

  readout(now: number): LatencyReadout {
    const fps = this.frames.filter((t) => now - t <= FPS_WINDOW_MS).length
    const m = median(this.samples)
    return { medianMs: m === null ? null : Math.round(m), samples: this.samples.length, fps }
  }

  reset(): void {
    this.probe = null
    this.lastSeq = 0
    this.lastFrameAt = -Infinity
    this.frames.length = 0
    this.samples.length = 0
  }
}

/** The HUD's one line. */
export function latencyText(r: LatencyReadout): string {
  const lat = r.medianMs === null ? 'input → frame —' : `input → frame ${r.medianMs} ms (median of ${r.samples})`
  return `${lat} · ${r.fps} fps`
}
