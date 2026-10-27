// Mobile tab → daemon routes (prd-mobile-dev-v1 §3, §5; vs-live M-A2,
// M-A13). The daemon owns the workspace's sim session (one per workspace in
// v1): its record, lane, state and control hold. The tab reads the status
// and sends gestures; it never decides anything about the device itself.
//
// Mirrors Builder A's frozen contract, `k2-core::sim::api` (StatusResp,
// SimView, DevicesResp, BootReq, ScreenshotResp) and the types it uses
// (`lane::LaneProbe`, `lane::DeviceProfile`, `hold::HoldView`,
// `stream::SimState`). Every parse lives in this file and fails loud on a
// body it doesn't understand.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export const SIM_STATUS_ROUTE = 'sim/status'
export const SIM_DEVICES_ROUTE = 'sim/devices'
export const SIM_BOOT_ROUTE = 'sim/boot'
export const SIM_STOP_ROUTE = 'sim/stop'
export const SIM_SCREENSHOT_ROUTE = 'sim/screenshot'

/** `lane::LaneKind`. */
export type SimLaneId = 'android' | 'ios' | 'ios-node'

/** `lane::LaneNeed`: a step between usable and ready. */
export type SimLaneNeed =
  | { kind: 'licence'; id: string; title: string }
  | { kind: 'install'; what: string; bytes?: number }
  | { kind: 'owner-step'; what: string; command: string }

/** `lane::LaneProbe`. */
export interface SimLaneStatus {
  lane: SimLaneId
  /** Can run (maybe after the licence / first-use install). */
  usable: boolean
  /** Can boot now. */
  ready: boolean
  needs: SimLaneNeed[]
  /** One line for the status view ("/dev/kvm: permission denied …"). */
  detail?: string
}

/** `stream::SimState`. */
export type SimState = 'installing' | 'booting' | 'running' | 'stopping' | 'stopped' | 'failed'

/** `hold::HoldView` (tag `kind`). */
export type SimHold =
  | { kind: 'free' }
  | { kind: 'person'; by: string; ms: number }
  | { kind: 'takeover'; by: string; idleMs: number }

/** `lane::DeviceProfile`. */
export interface SimDeviceProfile {
  /** Stable id the lane understands (`pixel_9`). */
  id: string
  /** Shown to people ("Pixel 9"). */
  name: string
  /** Screen in points, portrait. */
  widthPt: number
  heightPt: number
  scale: number
  os: string
  /** Booted when no device is named. */
  default: boolean
}

/** `api::SimView`: one device's state, as every client sees it. */
export interface SimView {
  /** `s<n>`. */
  sim: string
  workspace: string
  workspaceHandle: string
  lane: SimLaneId
  state: SimState
  profile?: SimDeviceProfile
  appId?: string
  hold: SimHold
  viewers: number
  /** Why it failed / stopped (`idle`, `limit`, an error message). */
  reason?: string
}

/** `api::StatusResp`, plus the caller's control (see `canControl`). */
export interface SimStatus {
  lanes: SimLaneStatus[]
  sim: SimView | null
  /** Devices running on the box now. */
  running: number
  /** The workspace's "Allow mobile devices" agent switch. */
  agentsAllowed: boolean
  /** May THIS caller tap, type, boot and stop? Remote Connect Members watch
   *  only (Rosson 2026-10-27). Read from the daemon's `canControl` when it
   *  sends one; else null and the pane asks `auth/whoami`. */
  canControl: boolean | null
}

const LANES: ReadonlySet<string> = new Set(['android', 'ios', 'ios-node'])
const STATES: ReadonlySet<string> = new Set(['installing', 'booting', 'running', 'stopping', 'stopped', 'failed'])

function fail(what: string, raw: unknown): never {
  throw new Error(`unexpected ${what} response: ${JSON.stringify(raw)}`)
}

function isObj(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === 'object' && !Array.isArray(v)
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v ? v : undefined
}

function parseNeed(raw: unknown): SimLaneNeed {
  if (!isObj(raw)) fail('sim lane need', raw)
  if (raw.kind === 'licence' && typeof raw.id === 'string' && typeof raw.title === 'string') {
    return { kind: 'licence', id: raw.id, title: raw.title }
  }
  if (raw.kind === 'install' && typeof raw.what === 'string') {
    return { kind: 'install', what: raw.what, ...(typeof raw.bytes === 'number' ? { bytes: raw.bytes } : {}) }
  }
  if (raw.kind === 'owner-step' && typeof raw.what === 'string' && typeof raw.command === 'string') {
    return { kind: 'owner-step', what: raw.what, command: raw.command }
  }
  fail('sim lane need', raw)
}

function parseLane(raw: unknown): SimLaneStatus {
  if (
    !isObj(raw) ||
    typeof raw.lane !== 'string' ||
    !LANES.has(raw.lane) ||
    typeof raw.usable !== 'boolean' ||
    typeof raw.ready !== 'boolean'
  ) {
    fail('sim lane', raw)
  }
  const needs = raw.needs === undefined ? [] : Array.isArray(raw.needs) ? raw.needs.map(parseNeed) : fail('sim lane', raw)
  const detail = str(raw.detail)
  return { lane: raw.lane as SimLaneId, usable: raw.usable, ready: raw.ready, needs, ...(detail ? { detail } : {}) }
}

export function parseSimHold(raw: unknown): SimHold {
  if (!isObj(raw)) fail('sim hold', raw)
  if (raw.kind === 'free') return { kind: 'free' }
  if (raw.kind === 'person' && typeof raw.by === 'string' && typeof raw.ms === 'number') {
    return { kind: 'person', by: raw.by, ms: raw.ms }
  }
  // HoldView's struct fields carry no rename: `idle_ms` on the wire today.
  const idle = typeof raw.idleMs === 'number' ? raw.idleMs : raw.idle_ms
  if (raw.kind === 'takeover' && typeof raw.by === 'string' && typeof idle === 'number') {
    return { kind: 'takeover', by: raw.by, idleMs: idle }
  }
  fail('sim hold', raw)
}

export function parseDeviceProfile(raw: unknown): SimDeviceProfile {
  if (
    !isObj(raw) ||
    typeof raw.id !== 'string' ||
    typeof raw.name !== 'string' ||
    typeof raw.widthPt !== 'number' ||
    typeof raw.heightPt !== 'number' ||
    typeof raw.scale !== 'number' ||
    typeof raw.os !== 'string'
  ) {
    fail('sim device', raw)
  }
  return {
    id: raw.id,
    name: raw.name,
    widthPt: raw.widthPt,
    heightPt: raw.heightPt,
    scale: raw.scale,
    os: raw.os,
    default: raw.default === true,
  }
}

export function parseSimView(raw: unknown): SimView {
  if (
    !isObj(raw) ||
    typeof raw.sim !== 'string' ||
    typeof raw.workspace !== 'string' ||
    typeof raw.lane !== 'string' ||
    !LANES.has(raw.lane) ||
    typeof raw.state !== 'string' ||
    !STATES.has(raw.state)
  ) {
    fail('sim view', raw)
  }
  const appId = str(raw.appId)
  const reason = str(raw.reason)
  return {
    sim: raw.sim,
    workspace: raw.workspace,
    workspaceHandle: typeof raw.workspaceHandle === 'string' ? raw.workspaceHandle : '',
    lane: raw.lane as SimLaneId,
    state: raw.state as SimState,
    ...(raw.profile !== undefined && raw.profile !== null ? { profile: parseDeviceProfile(raw.profile) } : {}),
    ...(appId ? { appId } : {}),
    hold: parseSimHold(raw.hold),
    viewers: typeof raw.viewers === 'number' ? raw.viewers : 0,
    ...(reason ? { reason } : {}),
  }
}

export function parseSimStatus(raw: unknown): SimStatus {
  if (!isObj(raw) || !Array.isArray(raw.lanes) || typeof raw.agentsAllowed !== 'boolean') fail('sim status', raw)
  return {
    lanes: raw.lanes.map(parseLane),
    sim: raw.sim === undefined || raw.sim === null ? null : parseSimView(raw.sim),
    running: typeof raw.running === 'number' ? raw.running : 0,
    agentsAllowed: raw.agentsAllowed,
    canControl: typeof raw.canControl === 'boolean' ? raw.canControl : null,
  }
}

/** `api::DevicesResp`. */
export function parseSimDevices(raw: unknown): { lane: SimLaneId; devices: SimDeviceProfile[] } {
  if (!isObj(raw) || typeof raw.lane !== 'string' || !LANES.has(raw.lane) || !Array.isArray(raw.devices)) {
    fail('sim devices', raw)
  }
  return { lane: raw.lane as SimLaneId, devices: raw.devices.map(parseDeviceProfile) }
}

export async function fetchSimStatus(scope: ServerScope, workspace: string): Promise<SimStatus> {
  return parseSimStatus(await daemonCliGet<unknown>(scope, SIM_STATUS_ROUTE, { workspace }))
}

export async function fetchSimDevices(
  scope: ServerScope,
  workspace: string,
  platform?: SimLaneId,
): Promise<{ lane: SimLaneId; devices: SimDeviceProfile[] }> {
  return parseSimDevices(
    await daemonCliGet<unknown>(scope, SIM_DEVICES_ROUTE, { workspace, ...(platform ? { platform } : {}) }),
  )
}

/** Boot this workspace's device (`api::BootReq`). `noTab`: the person
 *  booted from a tab that is already open. */
export async function bootSim(
  scope: ServerScope,
  workspace: string,
  opts: { device?: string; platform?: SimLaneId } = {},
): Promise<SimView> {
  return parseSimView(
    await daemonCliPost<unknown>(scope, SIM_BOOT_ROUTE, {
      workspace,
      ...(opts.device ? { device: opts.device } : {}),
      ...(opts.platform ? { platform: opts.platform } : {}),
      noTab: true,
    }),
  )
}

export async function stopSim(scope: ServerScope, workspace: string): Promise<SimView> {
  return parseSimView(await daemonCliPost<unknown>(scope, SIM_STOP_ROUTE, { workspace }))
}

/** Save a PNG under `.k2/sim/shots/`; returns its path (`api::ScreenshotResp`). */
export async function screenshotSim(scope: ServerScope, workspace: string): Promise<string> {
  const raw = await daemonCliPost<unknown>(scope, SIM_SCREENSHOT_ROUTE, { workspace })
  if (!isObj(raw) || typeof raw.path !== 'string' || !raw.path) fail('sim screenshot', raw)
  return raw.path
}

/** The lane a new device boots on: the first usable one. */
export function firstUsableLane(lanes: readonly SimLaneStatus[]): SimLaneStatus | null {
  return lanes.find((l) => l.usable) ?? null
}

/** A device that occupies the screen area (not stopped / failed / none). */
export function simIsUp(view: SimView | null): boolean {
  return !!view && (view.state === 'installing' || view.state === 'booting' || view.state === 'running' || view.state === 'stopping')
}
