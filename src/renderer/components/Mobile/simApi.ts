// Mobile tab → daemon routes (prd-mobile-dev-v1 §3, §5; vs-live M-A2,
// M-A13). The daemon owns the workspace's sim session (one per workspace in
// v1): its record, lane, state and control hold. The tab reads the status
// and sends gestures; it never decides anything about the device itself.
//
// ADAPTER: the route shapes below follow the PRD / vs-live until Builder A
// freezes `/cli/sim/*`. Every parse lives in this file, so a frozen shape
// changes only here. Parses fail loud on a body they don't understand.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export const SIM_STATUS_ROUTE = 'sim/status'
export const SIM_DEVICES_ROUTE = 'sim/list-devices'
export const SIM_BOOT_ROUTE = 'sim/boot'
export const SIM_STOP_ROUTE = 'sim/stop'
export const SIM_SCREENSHOT_ROUTE = 'sim/screenshot'

/** A lane (§3.1). Only the lane-blind core is drawn here (M-A21). */
export type SimLaneId = 'android' | 'ios' | 'ios-node'

export interface SimLaneStatus {
  lane: SimLaneId
  usable: boolean
  /** Why not, in plain words ("/dev/kvm is not usable by K2"). */
  reason?: string
}

export type SimState = 'off' | 'booting' | 'running' | 'stopping' | 'error'
export type SimOrientation = 'portrait' | 'landscape'

/** Who holds the device right now (§5). */
export interface SimControl {
  /** `person` = a person's tap (3 s rolling); `takeover` = Take over. */
  heldBy: 'person' | 'takeover' | null
  /** Who, when the daemon says ("rosson"). */
  by?: string
}

export interface SimSession {
  id: string
  lane: SimLaneId
  /** The device profile's name ("Pixel 9"). */
  device: string
  state: SimState
  orientation: SimOrientation
  /** The screen in dp / points, portrait-first (the stream's frame size). */
  screen: { width: number; height: number } | null
  /** The foreground app, when known ("com.acme.app"). */
  app?: string
  control: SimControl
  /** Why the device is in `error`. */
  error?: string
}

export interface SimStatus {
  lanes: SimLaneStatus[]
  session: SimSession | null
  /** May THIS caller tap, type, boot and stop? Remote Connect Members
   *  watch only (Rosson 2026-10-27); the daemon answers per login. */
  canControl: boolean
}

export interface SimDeviceProfile {
  /** Stable id the boot route takes. */
  id: string
  /** Shown in the chooser ("Pixel 9"). */
  name: string
  lane: SimLaneId
}

const LANES: ReadonlySet<string> = new Set(['android', 'ios', 'ios-node'])
const STATES: ReadonlySet<string> = new Set(['off', 'booting', 'running', 'stopping', 'error'])

function fail(what: string, raw: unknown): never {
  throw new Error(`unexpected ${what} response: ${JSON.stringify(raw)}`)
}

function isObj(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}

function parseLane(raw: unknown): SimLaneStatus {
  if (!isObj(raw) || typeof raw.lane !== 'string' || !LANES.has(raw.lane) || typeof raw.usable !== 'boolean') {
    fail('sim lane', raw)
  }
  return {
    lane: raw.lane as SimLaneId,
    usable: raw.usable,
    ...(typeof raw.reason === 'string' && raw.reason ? { reason: raw.reason } : {}),
  }
}

function parseScreen(raw: unknown): SimSession['screen'] {
  if (raw === null || raw === undefined) return null
  if (!isObj(raw) || typeof raw.width !== 'number' || typeof raw.height !== 'number' || raw.width <= 0 || raw.height <= 0) {
    fail('sim screen', raw)
  }
  return { width: raw.width, height: raw.height }
}

function parseControl(raw: unknown): SimControl {
  if (raw === null || raw === undefined) return { heldBy: null }
  if (!isObj(raw)) fail('sim control', raw)
  const heldBy = raw.heldBy === 'person' || raw.heldBy === 'takeover' ? raw.heldBy : null
  return { heldBy, ...(typeof raw.by === 'string' && raw.by ? { by: raw.by } : {}) }
}

export function parseSimSession(raw: unknown): SimSession | null {
  if (raw === null || raw === undefined) return null
  if (
    !isObj(raw) ||
    typeof raw.id !== 'string' ||
    typeof raw.lane !== 'string' ||
    !LANES.has(raw.lane) ||
    typeof raw.device !== 'string' ||
    typeof raw.state !== 'string' ||
    !STATES.has(raw.state)
  ) {
    fail('sim session', raw)
  }
  return {
    id: raw.id,
    lane: raw.lane as SimLaneId,
    device: raw.device,
    state: raw.state as SimState,
    orientation: raw.orientation === 'landscape' ? 'landscape' : 'portrait',
    screen: parseScreen(raw.screen),
    ...(typeof raw.app === 'string' && raw.app ? { app: raw.app } : {}),
    control: parseControl(raw.control),
    ...(typeof raw.error === 'string' && raw.error ? { error: raw.error } : {}),
  }
}

export function parseSimStatus(raw: unknown): SimStatus {
  if (!isObj(raw) || !Array.isArray(raw.lanes) || typeof raw.canControl !== 'boolean') fail('sim status', raw)
  return {
    lanes: raw.lanes.map(parseLane),
    session: parseSimSession(raw.session),
    canControl: raw.canControl,
  }
}

export function parseSimDevices(raw: unknown): SimDeviceProfile[] {
  if (!isObj(raw) || !Array.isArray(raw.devices)) fail('sim devices', raw)
  return raw.devices.map((d) => {
    if (!isObj(d) || typeof d.id !== 'string' || typeof d.name !== 'string' || typeof d.lane !== 'string' || !LANES.has(d.lane)) {
      fail('sim device', d)
    }
    return { id: d.id, name: d.name, lane: d.lane as SimLaneId }
  })
}

export async function fetchSimStatus(scope: ServerScope, workspace: string): Promise<SimStatus> {
  return parseSimStatus(await daemonCliGet<unknown>(scope, SIM_STATUS_ROUTE, { workspace }))
}

export async function fetchSimDevices(scope: ServerScope, workspace: string): Promise<SimDeviceProfile[]> {
  return parseSimDevices(await daemonCliGet<unknown>(scope, SIM_DEVICES_ROUTE, { workspace }))
}

/** Boot this workspace's device. `noTab`: the person booted from a tab that
 *  is already open, so the daemon sends no `open_mobile` for it. */
export async function bootSim(
  scope: ServerScope,
  workspace: string,
  opts: { device?: string; platform?: SimLaneId } = {},
): Promise<SimSession> {
  const raw = await daemonCliPost<unknown>(scope, SIM_BOOT_ROUTE, {
    workspace,
    ...(opts.device ? { device: opts.device } : {}),
    ...(opts.platform ? { platform: opts.platform } : {}),
    noTab: true,
  })
  const session = isObj(raw) ? parseSimSession(raw.session) : null
  if (!session) fail('sim boot', raw)
  return session
}

export async function stopSim(scope: ServerScope, workspace: string): Promise<void> {
  const raw = await daemonCliPost<unknown>(scope, SIM_STOP_ROUTE, { workspace })
  if (!isObj(raw) || raw.ok !== true) fail('sim stop', raw)
}

/** Save a PNG under `.k2/sim/shots/`; returns its workspace path. */
export async function screenshotSim(scope: ServerScope, workspace: string): Promise<string> {
  const raw = await daemonCliPost<unknown>(scope, SIM_SCREENSHOT_ROUTE, { workspace })
  if (!isObj(raw) || typeof raw.path !== 'string' || !raw.path) fail('sim screenshot', raw)
  return raw.path
}

/** The lane a new device boots on: the first usable one. */
export function firstUsableLane(lanes: readonly SimLaneStatus[]): SimLaneStatus | null {
  return lanes.find((l) => l.usable) ?? null
}
