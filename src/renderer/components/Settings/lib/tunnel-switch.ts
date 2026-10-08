// Settings → K2 Connect: move THIS device to another subdomain (0.45.2).
//
// The daemon owns the switch (`POST /cli/tunnel/switch`): it binds the new
// name and moves the encrypted listener, its certificate and renewer, the
// nested routes and frpc, and rolls back to the old name if a step fails.
// The renderer only does what needs the k2.dev account session: claim the
// new name's lease FIRST (a refused claim changes nothing), and release the
// leases afterwards (the old one on success, the new one on failure).
//
// Before 0.45.2 the panel stopped the tunnel and overwrote tunnel.json
// before claiming, so a refused claim left the device with neither name,
// and the daemon's E2E listener kept serving the OLD certificate after a
// switch (`CertificateUnknown` for every direct client until a restart).

/** One step the daemon reports (`config`, `listener`, `routing`,
 *  `certificate`, `tunnel`, `rollback`). */
export interface TunnelSwitchStep {
  step: string
  ok: boolean
  detail: string
}

/** `POST /cli/tunnel/switch` response (also `identityChange` on config /
 *  release responses). */
export interface TunnelSwitchReport {
  ok: boolean
  changed: boolean
  previousSubdomain: string | null
  subdomain: string | null
  steps: TunnelSwitchStep[]
  error?: string
  rolledBack: boolean
  status?: { running: boolean; public_url?: string | null }
}

export type TunnelSwitchPhase = 'claim' | 'daemon' | 'done' | 'failed'

export interface TunnelSwitchProgress {
  phase: TunnelSwitchPhase
  /** Plain-language line for the panel. */
  message: string
  /** Daemon steps once the daemon answered. */
  steps: TunnelSwitchStep[]
}

export interface TunnelSwitchRequest {
  /** Current subdomain ('' when none is bound, e.g. after a release). */
  from: string
  to: string
  /** The new subdomain's tunnel token (from the account list). */
  token: string
  deviceId: string
  deviceLabel?: string
  /** Start (or keep) the tunnel on the new name. */
  start: boolean
}

export interface TunnelSwitchDeps {
  /** Claim the new name's lease; `null` when there is no account session
   *  (the daemon still switches; nothing to claim). */
  claim: ((label: string) => Promise<{ claimed: boolean; holder: string | null }>) | null
  /** Release a lease (best-effort; errors are ignored). */
  releaseLease: ((label: string) => Promise<void>) | null
  /** `POST /cli/tunnel/switch` with the JSON body. */
  postSwitch: (body: Record<string, unknown>) => Promise<Response>
}

export interface TunnelSwitchOutcome {
  ok: boolean
  /** Shown to the owner when `ok` is false. */
  error: string | null
  /** The daemon put the previous name back (`ok` false only). */
  rolledBack: boolean
  /** Nothing on the device changed (refused before the daemon ran). */
  unchanged: boolean
  report: TunnelSwitchReport | null
}

const host = (label: string): string => `${label}.k2.dev`

async function errorOf(res: Response): Promise<string> {
  const text = await res.text()
  try {
    const parsed = JSON.parse(text) as { error?: unknown }
    if (parsed && typeof parsed.error === 'string') return parsed.error
  } catch {
    /* raw text */
  }
  return text || `request failed (${res.status})`
}

function bestEffort(p: Promise<void> | undefined): void {
  if (p) void p.catch(() => undefined)
}

/** Run the switch. Never throws: the outcome says what happened. */
export async function runTunnelSwitch(
  req: TunnelSwitchRequest,
  deps: TunnelSwitchDeps,
  onProgress: (p: TunnelSwitchProgress) => void,
): Promise<TunnelSwitchOutcome> {
  const to = req.to.trim()
  const from = req.from.trim()
  const fail = (
    error: string,
    extra: Partial<TunnelSwitchOutcome> = {},
    steps: TunnelSwitchStep[] = [],
  ): TunnelSwitchOutcome => {
    onProgress({ phase: 'failed', message: error, steps })
    return { ok: false, error, rolledBack: false, unchanged: false, report: null, ...extra }
  }

  // 1. Claim the new name first: a refusal must leave everything as it is.
  if (deps.claim) {
    onProgress({ phase: 'claim', message: `Claiming ${host(to)} for this device…`, steps: [] })
    try {
      const r = await deps.claim(to)
      if (!r.claimed && r.holder && r.holder !== req.deviceId) {
        return fail(`${host(to)} is in use on another device — stop it there first.`, {
          unchanged: true,
        })
      }
    } catch (e) {
      return fail(e instanceof Error ? e.message : `Could not claim ${host(to)}`, {
        unchanged: true,
      })
    }
  }

  // 2. The daemon moves everything (or rolls back).
  onProgress({
    phase: 'daemon',
    message: req.start
      ? `Moving the tunnel to ${host(to)} (certificate, encrypted listener, relay)…`
      : `Binding ${host(to)} on this device…`,
    steps: [],
  })
  let res: Response
  try {
    res = await deps.postSwitch({
      subdomain: to,
      token: req.token,
      deviceId: req.deviceId,
      deviceLabel: req.deviceLabel ?? '',
      start: req.start,
    })
  } catch (e) {
    if (from !== to) bestEffort(deps.releaseLease?.(to))
    return fail(e instanceof Error ? e.message : 'The daemon did not answer the switch')
  }
  if (!res.ok) {
    // 400 = bad input; the daemon changed nothing.
    if (from !== to) bestEffort(deps.releaseLease?.(to))
    return fail(await errorOf(res), { unchanged: true })
  }
  const report = (await res.json()) as TunnelSwitchReport
  const steps = Array.isArray(report.steps) ? report.steps : []
  if (!report.ok) {
    if (from !== to) bestEffort(deps.releaseLease?.(to))
    const back = report.rolledBack
      ? from
        ? ` ${host(from)} was put back.`
        : ' The previous setup was put back.'
      : ''
    return fail(
      `Switch to ${host(to)} failed: ${report.error ?? 'unknown error'}.${back}`,
      { rolledBack: report.rolledBack, report },
      steps,
    )
  }

  // 3. Done: the old lease is no longer ours to hold.
  if (from && from !== to) bestEffort(deps.releaseLease?.(from))
  onProgress({
    phase: 'done',
    message: req.start ? `Now connected as ${host(to)}.` : `Bound ${host(to)} — Start the tunnel below.`,
    steps,
  })
  return { ok: true, error: null, rolledBack: false, unchanged: false, report }
}
