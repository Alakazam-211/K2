// Heartbeat S6 — Keep awake. The daemon owns the mode and says what is
// really held (`GET /cli/power/status`). This file only names the wire
// shape and picks an icon tone; every word the user reads comes from the
// daemon's `label` and `detail`.

export type KeepAwakeMode = 'off' | 'working' | 'always'

/** The daemon's fixed state vocabulary (power/keep_awake.rs `describe`). */
export type KeepAwakeState =
  | 'off'
  | 'waiting'
  | 'paused'
  | 'lid_closed_ok'
  | 'lid_open_only'
  | 'limited'
  | 'error'

export interface KeepAwakeStatus {
  mode: KeepAwakeMode
  state: KeepAwakeState
  /** Short, honest: "Awake (lid open only)", "Paused: battery below 20%". */
  label: string
  /** One sentence on why, with any steps. */
  detail: string
  held: boolean
  lidHeld: boolean
  workingSessions: number
  powerSource: { onAc: boolean | null; batteryPercent: number | null }
  batteryFloorPercent: number
  /** "Also on battery", shared with Wake this computer (D12). */
  alsoOnBattery: boolean
  /** macOS without the helper: "Allow lid closed" can show the dialog. */
  canApproveLid: boolean
  lidDialogDeclined: boolean
  platform: string
  /** Set on a POST: what happened (for example a declined dialog). */
  message?: string
}

export const KEEP_AWAKE_MODES: { mode: KeepAwakeMode; label: string }[] = [
  { mode: 'off', label: 'Off' },
  { mode: 'working', label: 'While agents are working' },
  { mode: 'always', label: 'Always' },
]

export type KeepAwakeTone = 'off' | 'armed' | 'held' | 'warn'

/** Icon tone: held (accent), armed (on but not holding now), warn
 *  (paused or refused), off (muted). */
export function keepAwakeTone(s: KeepAwakeStatus): KeepAwakeTone {
  if (s.state === 'off') return 'off'
  if (s.state === 'paused' || s.state === 'error') return 'warn'
  if (s.state === 'limited' && !s.held) return 'warn'
  if (s.held) return 'held'
  return 'armed'
}
