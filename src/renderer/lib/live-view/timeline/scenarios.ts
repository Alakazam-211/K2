// Every scenario of the live timeline's sample, in the mock's order: the
// watched edit first (the tab's default), then the monitor's states,
// preparing video, the pinned snapshots, the display, the turn, opening,
// the connection, presence, refusals, a hold gone outside, the four outside
// saves, the large timeline, and the empty and error states.

import { buildEmpty, buildOpen, buildPresence, buildRefuse, buildSaves } from './scenarios-v1'
import { buildLarge } from './scenarios-large'
import { buildGolden, buildPrep, buildStates } from './scenarios-monitor'
import { buildConn, buildDisplay, buildHoldGone, WATCH } from './scenarios-watch'
import type { Scenario } from './sim'
import { TURN } from './world'

export const STATES = buildStates()
export const PREP = buildPrep()
export const GOLDEN = buildGolden()
export const DISPLAY = buildDisplay()
export const OPEN = buildOpen()
export const CONN = buildConn()
export const PRES = buildPresence()
export const REFUSE = buildRefuse()
export const HOLDGONE = buildHoldGone()
export const SAVES = buildSaves()
export const LARGE_SC = buildLarge()
export const EMPTY = buildEmpty()

export const SCENARIOS: Scenario[] = [WATCH, STATES, PREP, GOLDEN, DISPLAY, TURN, OPEN, CONN, PRES, REFUSE, HOLDGONE, ...SAVES, LARGE_SC, ...EMPTY]

export { TURN, WATCH }
