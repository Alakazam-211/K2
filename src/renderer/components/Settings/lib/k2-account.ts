// k2-account.ts — K2 Connect account API (k2.dev / Supabase Auth).
//
// Pure fetch wrappers around the Supabase Auth + REST endpoints that back
// the k2.dev account. The desktop app uses these to sign the user in,
// list the subdomains they've purchased, and bind one to this device's
// tunnel config. No SDK — plain `fetch` against the verified REST shapes.
//
// The ANON key below is the PUBLIC key for the project. It is safe to ship
// in a client: it only authorizes the unauthenticated `anon` role, and all
// row access is scoped by Supabase RLS to the signed-in caller.

import { AIRGAP_TEACHING, isAirgap } from '@/lib/airgap'

const SUPABASE_URL = 'https://ttgcalfrzzgkxnfepkiu.supabase.co'

function refuseAirgap(): void {
  if (isAirgap()) throw new Error(AIRGAP_TEACHING)
}

const SUPABASE_ANON_KEY =
  'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InR0Z2NhbGZyenpna3huZmVwa2l1Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODA1MDIyMzksImV4cCI6MjA5NjA3ODIzOX0.L28xgtYkPEj5eCNDGO5Zf5xxhdKQLxKD8c1CJRHNqI8'

/** A subdomain the signed-in user owns under k2.dev. */
export interface K2Subdomain {
  /**
   * The bare **apex** label, e.g. `rosson` → exposes `rosson.k2.dev`.
   * Must not contain `.` — nested DNS (`staging.z3thon`) is routing under an
   * apex tunnel, not a separate frpc tunnel root.
   */
  label: string
  /** Provisioning state, e.g. `active` | `pending`. */
  status: string
  /** The frpc bearer token bound to this subdomain. SECRET. */
  tunnel_token: string
  /** Device id currently holding the live claim/lease, if any. */
  claimed_by?: string | null
  /** ISO timestamp of the last claim heartbeat, if any. */
  claimed_at?: string | null
  /** Human-readable label of the holding device, if any. */
  claimed_label?: string | null
}

/**
 * True when `label` is a tunnelable apex purchase (single DNS label under
 * `k2.dev`). Nested names (`api.z3thon`, `rosson.rpmavs`) are **not**
 * tunnel roots — they are routes under an apex tunnel.
 */
export function isApexTunnelLabel(label: string): boolean {
  const t = label.trim()
  if (!t) return false
  // No dots, no spaces; keep DNS-label-ish characters only.
  if (t.includes('.') || /\s/.test(t)) return false
  return /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/i.test(t)
}

/** Result of a `claim_subdomain` RPC. */
export interface K2ClaimResult {
  /** true = THIS device now holds the lease (also acts as a heartbeat). */
  claimed: boolean
  /** When `claimed:false`, the device id that currently holds it. */
  holder: string | null
  /** When `claimed:false`, the human label of the holding device. */
  holderLabel: string | null
}

/** An authenticated k2.dev session. The access token is short-lived and
 *  kept in memory; the refresh token is persisted to the OS keychain. */
export interface K2Session {
  accessToken: string
  refreshToken: string
  email: string
}

interface SupabaseTokenResponse {
  access_token?: string
  refresh_token?: string
  user?: { email?: string | null } | null
  error_description?: string
  error?: string
  msg?: string
  message?: string
}

/** Pull the best human-readable message out of a Supabase error body. */
function authErrorMessage(body: SupabaseTokenResponse, status: number): string {
  return (
    body.error_description ||
    body.msg ||
    body.message ||
    (typeof body.error === 'string' ? body.error : '') ||
    `Request failed (${status})`
  )
}

/** Map a Supabase token response into our K2Session shape, throwing if
 *  any required field is missing. */
function toSession(body: SupabaseTokenResponse, status: number): K2Session {
  if (!body.access_token || !body.refresh_token) {
    throw new Error(authErrorMessage(body, status))
  }
  return {
    accessToken: body.access_token,
    refreshToken: body.refresh_token,
    email: body.user?.email ?? '',
  }
}

/** Sign in with an email + password against Supabase Auth. */
export async function signIn(email: string, password: string): Promise<K2Session> {
  refuseAirgap()
  const res = await fetch(`${SUPABASE_URL}/auth/v1/token?grant_type=password`, {
    method: 'POST',
    headers: {
      apikey: SUPABASE_ANON_KEY,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ email, password }),
  })
  const body = (await res.json().catch(() => ({}))) as SupabaseTokenResponse
  if (!res.ok) throw new Error(authErrorMessage(body, res.status))
  return toSession(body, res.status)
}

/** Exchange a refresh token for a fresh session. The refresh token may
 *  rotate — callers MUST persist the returned `refreshToken`. */
export async function refreshSession(refreshToken: string): Promise<K2Session> {
  refuseAirgap()
  const res = await fetch(`${SUPABASE_URL}/auth/v1/token?grant_type=refresh_token`, {
    method: 'POST',
    headers: {
      apikey: SUPABASE_ANON_KEY,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ refresh_token: refreshToken }),
  })
  const body = (await res.json().catch(() => ({}))) as SupabaseTokenResponse
  if (!res.ok) throw new Error(authErrorMessage(body, res.status))
  return toSession(body, res.status)
}

/** List the subdomains the signed-in user owns. RLS scopes the rows to
 *  the caller. Client filters to **apex tunnel labels only** (no dots) so
 *  nested routing names never appear as tunnel-bind options even if the
 *  control plane returns them in the same table. */
export async function listSubdomains(accessToken: string): Promise<K2Subdomain[]> {
  refuseAirgap()
  const res = await fetch(
    `${SUPABASE_URL}/rest/v1/subdomains?select=label,status,tunnel_token,claimed_by,claimed_at,claimed_label`,
    {
      method: 'GET',
      headers: {
        apikey: SUPABASE_ANON_KEY,
        Authorization: `Bearer ${accessToken}`,
      },
    },
  )
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as SupabaseTokenResponse
    throw new Error(authErrorMessage(body, res.status))
  }
  const rows = (await res.json()) as unknown
  if (!Array.isArray(rows)) return []
  return (rows as K2Subdomain[]).filter((row) => isApexTunnelLabel(row?.label ?? ''))
}

/** Claim (or heartbeat) a subdomain lease for this device. A successful
 *  claim (`claimed:true`) means THIS device now holds the lease — re-calling
 *  refreshes it. `claimed:false` means a *different* device holds a fresh
 *  claim (the `holder` / `holderLabel` identify who). Claims auto-expire
 *  server-side after 3 minutes without a heartbeat. */
export async function claimSubdomain(
  accessToken: string,
  label: string,
  deviceId: string,
  deviceLabel?: string,
): Promise<K2ClaimResult> {
  refuseAirgap()
  if (!isApexTunnelLabel(label)) {
    throw new Error(
      'Only apex subdomains can be claimed as tunnels (e.g. rosson → rosson.k2.dev). Nested names are not tunnel roots.',
    )
  }
  const res = await fetch(`${SUPABASE_URL}/rest/v1/rpc/claim_subdomain`, {
    method: 'POST',
    headers: {
      apikey: SUPABASE_ANON_KEY,
      Authorization: `Bearer ${accessToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ p_label: label, p_device_id: deviceId, p_device_label: deviceLabel ?? null }),
  })
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as SupabaseTokenResponse
    throw new Error(authErrorMessage(body, res.status))
  }
  const rows = (await res.json()) as unknown
  const row = Array.isArray(rows) ? (rows[0] as Record<string, unknown> | undefined) : undefined
  return {
    claimed: !!row?.claimed,
    holder: (row?.holder as string | null) ?? null,
    holderLabel: (row?.holder_label as string | null) ?? null,
  }
}

/** Release this device's lease on a subdomain (best-effort). */
export async function releaseSubdomain(
  accessToken: string,
  label: string,
  deviceId: string,
): Promise<void> {
  refuseAirgap()
  const res = await fetch(`${SUPABASE_URL}/rest/v1/rpc/release_subdomain`, {
    method: 'POST',
    headers: {
      apikey: SUPABASE_ANON_KEY,
      Authorization: `Bearer ${accessToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ p_label: label, p_device_id: deviceId }),
  })
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as SupabaseTokenResponse
    throw new Error(authErrorMessage(body, res.status))
  }
}

/** True if `claimedAt` is within the last 3 minutes — i.e. a live lease
 *  (server-side claims expire after 3 min of no heartbeat). */
export function freshClaim(claimedAt: string | null | undefined): boolean {
  if (!claimedAt) return false
  const t = Date.parse(claimedAt)
  if (Number.isNaN(t)) return false
  return Date.now() - t < 3 * 60 * 1000
}

/** Best-effort sign-out. Revokes the session server-side; failures are
 *  swallowed (the local keychain clear is what actually logs the user
 *  out of the app). */
export async function signOut(accessToken: string): Promise<void> {
  if (isAirgap()) return
  try {
    await fetch(`${SUPABASE_URL}/auth/v1/logout`, {
      method: 'POST',
      headers: {
        apikey: SUPABASE_ANON_KEY,
        Authorization: `Bearer ${accessToken}`,
      },
    })
  } catch {
    /* best-effort — ignore */
  }
}
