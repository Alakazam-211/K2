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

// ── M0 (measurement, off by default): the photon probe, the client's own
// frame stages, and the daemon's `stats` ─────────────────────────────────
//
// The photon probe is tap-to-photon: from a pointer-down to the first drawn
// frame whose centre pixel differs from the one on screen at the tap (the
// lab's photon test app flips its centre on touch). It measures what the
// person sees, so it needs no clock on the device.

/** One pixel, RGB. */
export type Rgb = readonly [number, number, number]

/** A pixel counts as changed when a channel moves by more than this (JPEG
 *  noise on a still screen stays well under it). */
export const PHOTON_THRESHOLD = 48

export function pixelChanged(a: Rgb, b: Rgb, threshold = PHOTON_THRESHOLD): boolean {
  return Math.abs(a[0] - b[0]) > threshold || Math.abs(a[1] - b[1]) > threshold || Math.abs(a[2] - b[2]) > threshold
}

/** Nearest-rank percentile of `xs` (p in 0..1); null when empty. */
export function percentile(xs: readonly number[], p: number): number | null {
  if (xs.length === 0) return null
  const s = [...xs].sort((a, b) => a - b)
  const rank = Math.min(s.length, Math.max(1, Math.ceil(p * s.length)))
  return s[rank - 1]
}

export interface PhotonReadout {
  p50: number | null
  p95: number | null
  samples: number
}

export class PhotonProbe {
  private armed: { at: number; base: Rgb } | null = null
  private readonly samples: number[] = []

  constructor(private readonly window = 50) {}

  /** Armed: the next frames get their centre pixel sampled. */
  get waiting(): boolean {
    return this.armed !== null
  }

  /** A pointer-down at `now` with `base` on screen (null: no frame yet). */
  arm(now: number, base: Rgb | null): void {
    if (this.armed && now - this.armed.at <= TIMEOUT_MS) return
    this.armed = base ? { at: now, base } : null
  }

  /** A frame was drawn at `now` showing `px` at the centre. */
  frame(now: number, px: Rgb | null): void {
    const a = this.armed
    if (!a || !px) return
    if (now - a.at > TIMEOUT_MS) {
      this.armed = null
      return
    }
    if (!pixelChanged(a.base, px)) return
    this.armed = null
    this.samples.push(now - a.at)
    if (this.samples.length > this.window) this.samples.shift()
  }

  readout(): PhotonReadout {
    const r = (x: number | null): number | null => (x === null ? null : Math.round(x))
    return { p50: r(percentile(this.samples, 0.5)), p95: r(percentile(this.samples, 0.95)), samples: this.samples.length }
  }

  reset(): void {
    this.armed = null
    this.samples.length = 0
  }
}

/** Where a frame is on this side of the socket. */
export type FrameStage = 'recv' | 'decoded' | 'drawn'

/** The client's stage times per frame: socket → decoded → drawn. */
export class FrameStageClock {
  private readonly open = new Map<number, { recv?: number; decoded?: number }>()
  private readonly decode: number[] = []
  private readonly draw: number[] = []

  constructor(private readonly window = 120) {}

  mark(seq: number, stage: FrameStage, now: number): void {
    const o = this.open.get(seq) ?? {}
    if (stage === 'recv') o.recv = now
    else if (stage === 'decoded') o.decoded = now
    else {
      if (o.recv !== undefined && o.decoded !== undefined) {
        this.push(this.decode, o.decoded - o.recv)
        this.push(this.draw, now - o.decoded)
      }
      this.open.delete(seq)
      return
    }
    this.open.set(seq, o)
    // Frames that never draw (stale, held) don't pile up.
    if (this.open.size > 32) this.open.delete(this.open.keys().next().value as number)
  }

  private push(xs: number[], v: number): void {
    xs.push(v)
    if (xs.length > this.window) xs.shift()
  }

  /** Median ms socket → decoded and decoded → drawn; null before a frame. */
  readout(): { decodeMs: number | null; drawMs: number | null } {
    const m = (xs: number[]): number | null => {
      const v = median(xs)
      return v === null ? null : Math.round(v * 10) / 10
    }
    return { decodeMs: m(this.decode), drawMs: m(this.draw) }
  }

  reset(): void {
    this.open.clear()
    this.decode.length = 0
    this.draw.length = 0
  }
}

/** The daemon's `stats` message (stream::StreamStats). */
export interface SimStreamStats {
  frames: number
  stages: { stage: string; n: number; p50: number; p95: number; max: number }[]
  inputs: { id: number; recvMs: number; laneMs: number }[]
  hubCpuMs: number
  encoderCpuMs: number
}

/** The HUD's second line, or null while there is nothing to show. */
export function detailText(photon: PhotonReadout, client: { decodeMs: number | null; drawMs: number | null }, daemon: SimStreamStats | null): string | null {
  const parts: string[] = []
  if (photon.samples > 0) parts.push(`tap → photon ${photon.p50} / ${photon.p95} ms (p50 / p95 of ${photon.samples})`)
  if (daemon) {
    const total = daemon.stages.find((s) => s.stage === 'total')
    if (total) parts.push(`daemon capture → sent ${total.p50.toFixed(1)} ms`)
    const lane = median(daemon.inputs.map((i) => i.laneMs))
    if (lane !== null) parts.push(`input ${lane.toFixed(1)} ms`)
    parts.push(`hub ${daemon.hubCpuMs.toFixed(0)} · enc ${daemon.encoderCpuMs.toFixed(0)} ms CPU/s`)
  }
  if (client.decodeMs !== null) parts.push(`decode ${client.decodeMs} · draw ${client.drawMs ?? '—'} ms`)
  return parts.length ? parts.join(' · ') : null
}
