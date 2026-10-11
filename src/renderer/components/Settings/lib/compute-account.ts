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
import { listSubdomains, type K2Subdomain } from './k2-account'

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
