// The live timeline's sample source: a LiveSource that plays the mock's
// sample world (the sample turn and every scenario, folded by sim.ts) to
// the shared shell. The banner offers five of them; every step is a whole
// snapshot, worked out when it's first shown. Sample data only: the tab
// draws the "Sample data" banner, and nothing here reads the opened file.
//
// The data comes in through one adapter (adapter.ts): the sample's pages
// and frames here, K2 Cores' timeline pack frames (a KeyedWatch) in T7.

import type { PlayerScenario, PlayerStep } from '../shared/sample-player'
import { SampleShellSource, type SampleShellOptions } from '../shared/sample-shell'
import type { ShellSample } from '../shared/samples'
import { CONN, HOLDGONE, OPEN, SAVES, TURN, WATCH } from './scenarios'
import { simulate, type Scenario } from './sim'
import { SAMPLE_CLOCK0, timelineSnapshot, type TimelineSnapshot } from './snapshot'
import { CTX, FACTS } from './world'

/** A sample step's snapshot: scenario `sc` played to step `i`. */
export function sampleSnapshot(sc: Scenario, i: number): TimelineSnapshot {
  return timelineSnapshot(simulate(sc, i, CTX), { facts: FACTS, clock0: SAMPLE_CLOCK0, scenario: sc.id })
}

/** One banner step from a scenario's step, its snapshot worked out once, when first read. */
function lazyStep(sc: Scenario, i: number, label?: string): PlayerStep<TimelineSnapshot> {
  let memo: TimelineSnapshot | null = null
  return {
    label: label ?? sc.steps[i].label ?? `Step ${i + 1}`,
    get state(): TimelineSnapshot {
      return (memo ??= sampleSnapshot(sc, i))
    },
  }
}

const whole = (sc: Scenario, id: string, label: string): PlayerScenario<TimelineSnapshot> => ({ id, label, steps: sc.steps.map((_, i) => lazyStep(sc, i)) })

/** The banner's scenarios: what the panel shows in T2. Every mock scenario stays in `SCENARIOS` for the lanes and monitor. */
export function timelineBannerScenarios(): PlayerScenario<TimelineSnapshot>[] {
  const last = (sc: Scenario, label: string): PlayerStep<TimelineSnapshot> => lazyStep(sc, sc.steps.length - 1, label)
  const save = (id: string): Scenario => SAVES.find((s) => s.id === id)!
  return [
    whole(OPEN, 'open', 'First open'),
    whole(WATCH, 'watch', 'Watching an edit'),
    whole(TURN, 'turn', 'Two agents'),
    {
      id: 'saves',
      label: 'Outside saves',
      steps: [last(save('save-ingested'), 'Taken in'), last(save('save-both'), 'Kept both'), last(save('save-declined'), 'Declined'), last(save('save-missing'), 'Missing'), last(HOLDGONE, 'A hold ended outside')],
    },
    whole(CONN, 'connection', 'Connection'),
  ]
}

export function timelineSample(): ShellSample {
  return {
    kind: 'timeline',
    fileName: 'rough-cut',
    scenarios: timelineBannerScenarios(),
    reply: 'On it: I’ll try it there once Brook lets go of Interview B.',
  }
}

/** The sample timeline as a LiveSource; it opens on the watched edit. */
export class TimelineSampleSource extends SampleShellSource {
  constructor(o: SampleShellOptions = {}) {
    super(timelineSample(), { scenario: 'watch', autoplay: false, ...o })
  }

  /** The timeline beside the shell's snapshot (the lanes and monitor read it). */
  timeline(): TimelineSnapshot['timeline'] {
    return (this.snapshot() as TimelineSnapshot).timeline
  }
}

