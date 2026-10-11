// compute-account.ts — the one-click path of Settings → Compute → This
// computer (prd-k2-compute-nodes-v1, "same Connect account").
//
// When this computer and the chosen server sit on the same k2.dev account,
// the control plane can vouch for the pairing instead of the owner copying
// an enroll code: the app asks Connect for a short-lived enroll ticket for
// (server, this node's key) and hands it to the local daemon.
//
// The Connect route may not exist yet on a given control plane. Any 404,
// 405 or network failure means "not on this account yet": the sheet falls
// back to an enroll code. Never under air-gap.

import { AIRGAP_TEACHING, isAirgap } from '@/lib/airgap'
import { listSubdomains, supabaseRestGet, type K2Subdomain } from './k2-account'

/** Connect control plane route that mints compute enroll tickets. */
export const COMPUTE_ENROLL_TICKET_URL = 'https://connect.k2.dev/compute/enroll-ticket'

export const ONE_CLICK_UNAVAILABLE =
  "One-click isn't available on this account yet; use an enroll code"

export type EnrollTicketResult =
  | { ok: true; ticket: string }
  | { ok: false; unavailable: boolean; message: string }

export interface EnrollTicketRequest {
  /** The server's k2.dev label, e.g. `rosson` for rosson.k2.dev. */
  controllerSub: string
  /** This computer's node key fingerprint (`k2-node` status `node_fp`). */
  nodeFp: string
  nodeName: string
}

/** The account's servers that can act as a controller (active apex labels). */
export async function listAccountServers(accessToken: string): Promise<K2Subdomain[]> {
  const rows = await listSubdomains(accessToken)
  return rows.filter((r) => r.status === 'active')
}

/** Ask Connect for a one-time enroll ticket. Never throws: every failure is
 *  a result the sheet can show. */
export async function requestEnrollTicket(
  accessToken: string,
  req: EnrollTicketRequest,
): Promise<EnrollTicketResult> {
  if (isAirgap()) return { ok: false, unavailable: true, message: AIRGAP_TEACHING }
  let res: Response
  try {
    res = await fetch(COMPUTE_ENROLL_TICKET_URL, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${accessToken}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        controller_sub: req.controllerSub,
        node_fp: req.nodeFp,
        node_name: req.nodeName,
      }),
    })
  } catch {
    return { ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE }
  }
  if (res.status === 404 || res.status === 405) {
    return { ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE }
  }
  const text = await res.text()
  let body: Record<string, unknown> = {}
  try {
    const parsed = JSON.parse(text) as unknown
    if (parsed && typeof parsed === 'object') body = parsed as Record<string, unknown>
  } catch {
    /* non-JSON body: handled below */
  }
  if (!res.ok) {
    const err = body.error
    const message =
      (typeof body.message === 'string' && body.message) ||
      (typeof err === 'string' && err) ||
      (err && typeof err === 'object' && typeof (err as { hint?: unknown }).hint === 'string'
        ? ((err as { hint: string }).hint)
        : '') ||
      `Connect refused the enroll ticket (${res.status})`
    return { ok: false, unavailable: false, message }
  }
  if (typeof body.ticket !== 'string' || !body.ticket) {
    return { ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE }
  }
  return { ok: true, ticket: body.ticket }
}

// ── Machines you can use (Settings → Compute, Rosson 2026-10-11) ────────
//
// A server with "Make this computer a node" on advertises itself to K2
// Connect (its daemon, `compute_advertise`); the account's machines are a
// Supabase table the signed-in owner reads through RLS. Switching one on
// asks Connect to mint a one-click ticket for THIS controller and deliver
// it to that machine (`deliver_to_sub`); the machine's daemon applies it
// on its next check-in. Spec: compute-oneclick-k2-connect-spec-v2.md.

/** Shown while the account side isn't there yet (table or route missing). */
export const ONE_CLICK_NEEDS_UPDATE =
  'One-click needs a K2 Connect update; meanwhile add a machine with an enroll code'

/** A machine advertises at least every few minutes; older = offline. */
export const MACHINE_ONLINE_SECS = 300

/** One row of `compute_machines` (snake_case, as stored). */
export interface ComputeMachine {
  sub: string
  node_fp: string
  node_name: string | null
  /** unenrolled | pending | connecting | online | offline | revoked */
  node_state: string | null
  /** Host of the controller it is enrolled with, e.g. `rosson.k2.dev`. */
  controller: string | null
  control: string | null
  /** The 6 digits the machine shows while pending (owner-only via RLS). */
  sas: string | null
  os: string | null
  arch: string | null
  cpu_cores: number | null
  mem_bytes: number | null
  disk_total_bytes: number | null
  disk_free_bytes: number | null
  node_version: string | null
  k2_version: string | null
  updated_at: string | null
}

export const COMPUTE_MACHINE_COLUMNS =
  'sub,node_fp,node_name,node_state,controller,control,sas,os,arch,cpu_cores,mem_bytes,disk_total_bytes,disk_free_bytes,node_version,k2_version,updated_at'

export type MachinesResult =
  | { ok: true; machines: ComputeMachine[] }
  /** `unavailable`: the table isn't on this project yet (capability check). */
  | { ok: false; unavailable: boolean; message: string }

/** PostgREST's "no such table" answers (404, PGRST205 / 42P01). */
function isMissingTable(status: number, body: Record<string, unknown>): boolean {
  if (status === 404) return true
  const code = typeof body.code === 'string' ? body.code : ''
  return code === 'PGRST205' || code === '42P01'
}

/** The account's machines that offer compute. Never throws. */
export async function listComputeMachines(accessToken: string): Promise<MachinesResult> {
  if (isAirgap()) return { ok: false, unavailable: true, message: AIRGAP_TEACHING }
  let res: Response
  try {
    res = await supabaseRestGet(`compute_machines?select=${COMPUTE_MACHINE_COLUMNS}&order=sub.asc`, accessToken)
  } catch (err) {
    return { ok: false, unavailable: false, message: `Couldn't read your machines: ${err instanceof Error ? err.message : String(err)}` }
  }
  const text = await res.text()
  let parsed: unknown = null
  try {
    parsed = text ? (JSON.parse(text) as unknown) : null
  } catch {
    parsed = null
  }
  if (!res.ok) {
    const body = parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {}
    if (isMissingTable(res.status, body)) return { ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE }
    const msg = typeof body.message === 'string' ? body.message : `HTTP ${res.status}`
    return { ok: false, unavailable: false, message: `Couldn't read your machines: ${msg}` }
  }
  if (!Array.isArray(parsed)) return { ok: true, machines: [] }
  const machines = (parsed as Partial<ComputeMachine>[])
    .filter((m) => typeof m?.sub === 'string' && typeof m?.node_fp === 'string')
    .map((m) => ({
      sub: m.sub as string,
      node_fp: (m.node_fp as string).toLowerCase(),
      node_name: m.node_name ?? null,
      node_state: m.node_state ?? null,
      controller: m.controller ?? null,
      control: m.control ?? null,
      sas: m.sas ?? null,
      os: m.os ?? null,
      arch: m.arch ?? null,
      cpu_cores: m.cpu_cores ?? null,
      mem_bytes: m.mem_bytes ?? null,
      disk_total_bytes: m.disk_total_bytes ?? null,
      disk_free_bytes: m.disk_free_bytes ?? null,
      node_version: m.node_version ?? null,
      k2_version: m.k2_version ?? null,
      updated_at: m.updated_at ?? null,
    }))
  return { ok: true, machines }
}

/** Advertised within the last few minutes. */
export function machineOnline(m: Pick<ComputeMachine, 'updated_at'>, nowMs: number = Date.now()): boolean {
  if (!m.updated_at) return false
  const at = Date.parse(m.updated_at)
  return Number.isFinite(at) && nowMs - at <= MACHINE_ONLINE_SECS * 1000
}

export interface EnrollOfferRequest {
  /** This controller's label (`rosson` for rosson.k2.dev). */
  controllerSub: string
  /** The machine's label: Connect delivers the ticket to it. */
  nodeSub: string
  nodeFp: string
  nodeName: string
}

/** Ask Connect to mint a ticket for this controller and deliver it to the
 *  machine. Same fallbacks as `requestEnrollTicket`; never throws. */
export async function requestEnrollOffer(accessToken: string, req: EnrollOfferRequest): Promise<EnrollTicketResult> {
  if (isAirgap()) return { ok: false, unavailable: true, message: AIRGAP_TEACHING }
  let res: Response
  try {
    res = await fetch(COMPUTE_ENROLL_TICKET_URL, {
      method: 'POST',
      headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({
        controller_sub: req.controllerSub,
        node_fp: req.nodeFp,
        node_name: req.nodeName,
        deliver_to_sub: req.nodeSub,
      }),
    })
  } catch {
    return { ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE }
  }
  if (res.status === 404 || res.status === 405) {
    return { ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE }
  }
  const text = await res.text()
  let body: Record<string, unknown> = {}
  try {
    const parsed = JSON.parse(text) as unknown
    if (parsed && typeof parsed === 'object') body = parsed as Record<string, unknown>
  } catch {
    /* non-JSON */
  }
  if (!res.ok) {
    const err = body.error
    const message =
      (typeof body.message === 'string' && body.message) ||
      (typeof err === 'string' && err) ||
      `Connect refused the request (${res.status})`
    return { ok: false, unavailable: false, message }
  }
  if (body.delivered !== true || typeof body.ticket !== 'string') {
    // Minted but not delivered: the route predates delivery.
    return { ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE }
  }
  return { ok: true, ticket: body.ticket }
}
