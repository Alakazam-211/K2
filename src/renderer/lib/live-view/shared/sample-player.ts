// The generic sample player: a list of scenarios, each a list of steps, and
// play, pause, step and start over. The "Sample data" banner drives it. Each
// live view's sample (scene, timeline; later the workbook's) is scenarios of
// its own state; the player only walks them.

export interface PlayerStep<S> {
  label: string
  state: S
}

export interface PlayerScenario<S> {
  id: string
  label: string
  /** A heading for the banner's menu when there are many (the scene's 24). */
  group?: string
  steps: readonly PlayerStep<S>[]
}

export interface PlayerStatus {
  playing: boolean
  /** 1-based. */
  position: number
  length: number
  label: string
  scenario: string
}

export interface PlayerOptions<S> {
  scenarios: readonly PlayerScenario<S>[]
  scenario?: string
  autoplay?: boolean
  /** Time between steps while playing. */
  stepMs?: number
  /** Start over after the last step while playing. */
  loop?: boolean
}

export class SamplePlayer<S> {
  private readonly scenarios: readonly PlayerScenario<S>[]
  private sc: PlayerScenario<S>
  private i = 0
  private playing: boolean
  private timer: ReturnType<typeof setTimeout> | null = null
  private readonly stepMs: number
  private readonly loop: boolean
  private readonly stateSubs = new Set<(s: S) => void>()
  private readonly statusSubs = new Set<(s: PlayerStatus) => void>()
  private closed = false

  constructor(o: PlayerOptions<S>) {
    if (o.scenarios.length === 0) throw new Error('a sample player needs a scenario')
    this.scenarios = o.scenarios
    this.sc = o.scenarios.find((s) => s.id === o.scenario) ?? o.scenarios[0]
    this.stepMs = o.stepMs ?? 2500
    this.loop = o.loop ?? false
    this.playing = o.autoplay ?? true
    this.schedule()
  }

  current(): S {
    return this.sc.steps[this.i].state
  }

  status(): PlayerStatus {
    return { playing: this.playing, position: this.i + 1, length: this.sc.steps.length, label: this.sc.steps[this.i].label, scenario: this.sc.id }
  }

  scenarioList(): readonly { id: string; label: string; group?: string }[] {
    return this.scenarios.map((s) => (s.group ? { id: s.id, label: s.label, group: s.group } : { id: s.id, label: s.label }))
  }

  onState(cb: (s: S) => void): () => void {
    this.stateSubs.add(cb)
    return () => this.stateSubs.delete(cb)
  }

  onStatus(cb: (s: PlayerStatus) => void): () => void {
    this.statusSubs.add(cb)
    return () => this.statusSubs.delete(cb)
  }

  play(): void {
    if (this.closed) return
    if (this.i >= this.sc.steps.length - 1 && !this.loop) this.go(0)
    this.playing = true
    this.schedule()
    this.tellStatus()
  }

  pause(): void {
    this.playing = false
    this.clear()
    this.tellStatus()
  }

  /** One step on; at the end it stays (or starts over when looping). */
  step(): void {
    if (this.i < this.sc.steps.length - 1) this.go(this.i + 1)
    else if (this.loop) this.go(0)
  }

  restart(): void {
    this.go(0)
  }

  /** Straight to a step (0-based) of this scenario. */
  goTo(i: number): void {
    if (i >= 0 && i < this.sc.steps.length) this.go(i)
  }

  setScenario(id: string): void {
    const next = this.scenarios.find((s) => s.id === id)
    if (!next) return
    this.sc = next
    this.go(0, true)
  }

  close(): void {
    this.closed = true
    this.clear()
    this.stateSubs.clear()
    this.statusSubs.clear()
  }

  private go(i: number, force = false): void {
    if (i === this.i && !force) return
    this.i = i
    for (const cb of this.stateSubs) cb(this.current())
    this.schedule()
    this.tellStatus()
  }

  private schedule(): void {
    this.clear()
    if (!this.playing || this.closed) return
    this.timer = setTimeout(() => {
      this.timer = null
      if (this.i < this.sc.steps.length - 1 || this.loop) this.step()
      else {
        this.playing = false
        this.tellStatus()
      }
    }, this.stepMs)
  }

  private clear(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
  }

  private tellStatus(): void {
    const s = this.status()
    for (const cb of this.statusSubs) cb(s)
  }
}
