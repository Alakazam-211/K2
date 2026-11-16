// A LiveSource that plays a sample world (samples.ts) through the generic
// player: each step is a whole snapshot. Retry jumps to the connection
// scenario's "trying" step; a message sends (or fails, on a step that fails
// sends), and the sample's agent answers a moment later. Sample data only:
// the tab draws the "Sample data" banner for `kind: 'sample'`.

import { SamplePlayer } from './sample-player'
import { isDisconnected } from './state'
import type { SampleSnapshot, ShellSample } from './samples'
import type { LiveMessage, LiveShellSnapshot, LiveSource, SayResult } from './source'

export interface SampleShellOptions {
  autoplay?: boolean
  scenario?: string
  stepMs?: number
  /** How long the sample's agent takes to answer. */
  replyMs?: number
}

export class SampleShellSource implements LiveSource {
  readonly kind = 'sample' as const
  readonly player: SamplePlayer<SampleSnapshot>
  readonly sample: ShellSample
  private readonly subs = new Set<(s: LiveShellSnapshot) => void>()
  private extra: LiveMessage[] = []
  private snap: SampleSnapshot
  private readonly replyMs: number
  private timers = new Set<ReturnType<typeof setTimeout>>()
  private seq = 0

  constructor(sample: ShellSample, o: SampleShellOptions = {}) {
    this.sample = sample
    this.replyMs = o.replyMs ?? 1500
    this.player = new SamplePlayer({ scenarios: sample.scenarios, scenario: o.scenario ?? 'open', autoplay: o.autoplay ?? false, stepMs: o.stepMs })
    this.snap = this.merge()
    this.player.onState(() => this.changed())
  }

  snapshot(): SampleSnapshot {
    return this.snap
  }

  subscribe(cb: (s: LiveShellSnapshot) => void): () => void {
    this.subs.add(cb)
    return () => this.subs.delete(cb)
  }

  retry(): void {
    const sc = this.player.status().scenario
    const steps = this.sample.scenarios.find((s) => s.id === sc)?.steps ?? []
    const at = steps.findIndex((s) => s.state.connection === 'trying')
    if (at >= 0 && this.snap.connection !== 'trying' && isDisconnected(this.snap.connection)) this.player.goTo(at)
  }

  say(msg: { to: string; text: string; address: string | null }): Promise<SayResult> {
    const cur = this.player.current()
    if (cur.hints?.sayFails || isDisconnected(cur.connection)) return Promise.resolve({ ok: false })
    const now = cur.clock ?? Date.now()
    this.extra = [...this.extra, { id: `say-${++this.seq}`, from: cur.self, to: msg.to, text: msg.text, at_ms: now }]
    this.changed()
    const t = setTimeout(() => {
      this.timers.delete(t)
      this.extra = [...this.extra, { id: `reply-${++this.seq}`, from: msg.to, to: cur.self, text: this.sample.reply, at_ms: now }]
      this.changed()
    }, this.replyMs)
    this.timers.add(t)
    return Promise.resolve({ ok: true })
  }

  close(): void {
    for (const t of this.timers) clearTimeout(t)
    this.timers.clear()
    this.subs.clear()
    this.player.close()
  }

  private merge(): SampleSnapshot {
    const cur = this.player.current()
    return this.extra.length ? { ...cur, messages: [...cur.messages, ...this.extra] } : cur
  }

  private changed(): void {
    this.snap = this.merge()
    for (const cb of this.subs) cb(this.snap)
  }
}
