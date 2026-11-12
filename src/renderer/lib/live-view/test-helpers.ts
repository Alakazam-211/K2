// Test helpers: fold the sample's steps through the reducer and keep the
// state after each step and the book at each rev. Used by the golden checks
// and the reducer tests only.

import { initialLiveState, liveWorkbookReducer, type LiveWorkbookAction, type LiveWorkbookState } from './reducer'
import { sampleSteps, type SampleScenario, type SampleStep } from './sample-source'
import type { Book } from './workbook'

export interface Played {
  steps: SampleStep[]
  /** The state after step i. */
  states: LiveWorkbookState[]
  /** The book at each rev the sample reaches. */
  books: Record<number, Book>
}

export function fold(actions: readonly LiveWorkbookAction[], from: LiveWorkbookState = initialLiveState()): LiveWorkbookState {
  return actions.reduce(liveWorkbookReducer, from)
}

export function play(scenario: SampleScenario = 'turn'): Played {
  const steps = sampleSteps(scenario)
  const states: LiveWorkbookState[] = []
  const books: Record<number, Book> = {}
  let s = initialLiveState()
  for (const st of steps) {
    s = fold(st.events, s)
    if (s.book) books[s.book.rev] = s.book
    states.push(s)
  }
  return { steps, states, books }
}

/** The state after the last step whose book is at `rev`. */
export function stateAtRev(p: Played, rev: number): LiveWorkbookState {
  for (let i = p.states.length - 1; i >= 0; i--) if (p.states[i].book?.rev === rev) return p.states[i]
  throw new Error(`the sample never reaches rev ${rev}`)
}
