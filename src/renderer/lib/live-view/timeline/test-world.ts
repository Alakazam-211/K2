// Helpers for the timeline's model checks (the mock's 89, as vitest): the
// sample world, folded the same way the sample source folds it.

import { expect } from 'vitest'
import { builtOf } from './diff'
import type { Book, TimelineFrame } from './schema'
import { simulate, simulateRaw, type Scenario, type SimState } from './sim'
import { CTX } from './world'

export const sim = (sc: Scenario, i: number): SimState => simulate(sc, i, CTX)
export const raw = (sc: Scenario, i: number): SimState => simulateRaw(sc, i, CTX)
export const at = (sc: Scenario, label: string): number => sc.steps.findIndex((s) => s.label === label)
export const X = (sc: Scenario, label: string): SimState => sim(sc, at(sc, label))
export const afterOf = (f: TimelineFrame): Book => builtOf(f)!.after
export const beforeOf = (f: TimelineFrame): Book => builtOf(f)!.before

/** A check passes: `what` says what it proves, and shows when it doesn't. */
export function ok(cond: unknown, what: string): void {
  expect(!!cond, what).toBe(true)
}
