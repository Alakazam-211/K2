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

export type KeepAwakeLidSetup = 'ready' | 'needs_setup' | 'unavailable' | 'unknown'

function lidSetupOf(v: unknown): KeepAwakeLidSetup {
  return v === 'ready' || v === 'needs_setup' || v === 'unavailable' ? v : 'unknown'
}

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
  /** "Also with the lid closed", the saved switch. Off by default. */
  lidClosed: boolean
  /** This OS has the switch (false on Windows: the power plan decides). */
  lidSwitch: boolean
  /** Is lid closed set up on the host (macOS: the power helper)? */
  lidSetup: KeepAwakeLidSetup
  /** Why it is not ready, or who can set it up, in the daemon's words. */
  lidSetupDetail: string
  /** This client may run Set up: an Admin or Owner at the host itself. */
  canSetUp: boolean
  /** 0.43.0/0.43.1 wire; the daemon keeps it equal to `canSetUp`. */
  canApproveLid: boolean
  /** 0.43.2 Q3: may this caller change Keep awake (an Admin or Owner; the
   *  route floor). A daemon before 0.43.2 leaves it out; its floor was
   *  Member, so absent reads true. */
  canChange: boolean
  lidDialogDeclined: boolean
  platform: string
  /** Set on a POST: what happened (for example a declined dialog). */
  message?: string
}

/**
 * `{ keepAwake }` from GET /cli/power/status or POST /cli/power/keep-awake,
 * or null when the body is not that shape (an error envelope, `{}`, or
 * another route's body). Null keeps the button hidden instead of crashing
 * on `status.mode`.
 */
export function parseKeepAwakeBody(raw: unknown): KeepAwakeStatus | null {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) return null
  const k = (raw as { keepAwake?: unknown }).keepAwake
  if (k === null || typeof k !== 'object' || Array.isArray(k)) return null
  const s = k as Record<string, unknown>
  if (s.mode !== 'off' && s.mode !== 'working' && s.mode !== 'always') return null
  if (typeof s.state !== 'string' || typeof s.label !== 'string') return null
  const ps =
    s.powerSource !== null && typeof s.powerSource === 'object'
      ? (s.powerSource as Record<string, unknown>)
      : {}
  return {
    mode: s.mode,
    state: s.state as KeepAwakeState,
    label: s.label,
    detail: typeof s.detail === 'string' ? s.detail : '',
    held: s.held === true,
    lidHeld: s.lidHeld === true,
    workingSessions: typeof s.workingSessions === 'number' ? s.workingSessions : 0,
    powerSource: {
      onAc: typeof ps.onAc === 'boolean' ? ps.onAc : null,
      batteryPercent: typeof ps.batteryPercent === 'number' ? ps.batteryPercent : null,
    },
    batteryFloorPercent: typeof s.batteryFloorPercent === 'number' ? s.batteryFloorPercent : 0,
    alsoOnBattery: s.alsoOnBattery === true,
    lidClosed: s.lidClosed === true,
    lidSwitch: s.lidSwitch === true,
    lidSetup: lidSetupOf(s.lidSetup),
    lidSetupDetail: typeof s.lidSetupDetail === 'string' ? s.lidSetupDetail : '',
    canSetUp: s.canSetUp === true,
    canApproveLid: s.canApproveLid === true,
    canChange: s.canChange !== false,
    lidDialogDeclined: s.lidDialogDeclined === true,
    platform: typeof s.platform === 'string' ? s.platform : '',
    ...(typeof s.message === 'string' ? { message: s.message } : {}),
  }
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
