// Settings → Email Hosting → a mailbox → Forwarding
// (prd-hostmail-external-forwarding-v1 §4.2, FW4). Typed bindings for the
// daemon's forward routes (crates/k2-daemon/src/mail/forward.rs). Every rule
// (who may set, resume, the confirm link, caps, auto-pause) lives in the
// daemon; this file only calls the routes.
//
//   staff  = /cli/mail/forward*       owner, Admin (and mail-manage agents)
//   person = /cli/mail/self/forward*  the mailbox's own person (a box login
//                                      linked to it, or the owner)

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { mailErrorInfo } from './email-api'

/** Which door the viewer came through. */
export type ForwardViewer = 'staff' | 'person'

export type ForwardState = 'pending_verify' | 'active' | 'paused'

export type PauseReason =
  | 'dest_gone'
  | 'refused'
  | 'revoked'
  | 'daily_cap'
  | 'box_brake'
  | 'receive_only'
  | 'air_gap'
  | 'upgrade_required'
  | 'manual'

export interface ForwardVerify {
  /** `pending` | `verified` | `expired` | `revoked` (email_verify states). */
  state: string
  /** Unix seconds; only while pending. */
  expiresAt: number | null
  verifiedAt: number | null
}

export interface ForwardCounts {
  forwarded: number
  delivered: number
  refused: number
  refusedNoSuchUser?: number
  refusedPolicy?: number
}

export interface Forward {
  id: string
  address: string
  to: string
  kind: 'internal' | 'external' | string
  keep: boolean
  state: ForwardState | string
  pauseReason: PauseReason | string | null
  /** The pause reason in words (the daemon's). */
  pauseWords: string | null
  setBy: string
  createdAt: number
  updatedAt: number
  activatedAt: number | null
  pausedAt: number | null
  verify: ForwardVerify | null
  last24h: ForwardCounts
}

export interface ForwardEvent {
  id: string
  forwardId: string
  /** set | verify_sent | verified | activated | paused | resumed | removed | replaced | drift | imported | keep_changed */
  event: string
  /** owner | agent:<name> | person:<key> | k2 */
  actor: string
  detail: Record<string, unknown> | null
  at: number
}

/** GET /cli/mail/forward?address= (and /cli/mail/self/forward). */
export interface ForwardShow {
  ok: boolean
  address: string
  forward: Forward | null
  events: ForwardEvent[]
}

/** POST set: 202 `pending_verify` (outside) or 200 `active`. */
export interface ForwardSetResult {
  ok: boolean
  address: string
  to: string
  keep: boolean
  kind: string
  state: string
  sentVia?: string | null
  message?: string
  note?: string | null
}

export interface ForwardChangeResult {
  ok: boolean
  address: string
  state?: string
  pauseReason?: string | null
  removed?: boolean
  note?: string | null
}

function base(viewer: ForwardViewer): string {
  return viewer === 'person' ? 'mail/self/forward' : 'mail/forward'
}

export function forwardApiOn(scope: ServerScope = primaryScope()) {
  return {
    show(viewer: ForwardViewer, address: string): Promise<ForwardShow> {
      return daemonCliGet<ForwardShow>(scope, base(viewer), { address })
    },
    set(viewer: ForwardViewer, address: string, to: string, keep: boolean | undefined): Promise<ForwardSetResult> {
      const body: Record<string, unknown> = { address, to }
      // keep rides only when the viewer chose it; the daemon defaults
      // outside forwards to keeping a copy, same-box ones to not.
      if (keep !== undefined) body.keep = keep
      return daemonCliPost<ForwardSetResult>(scope, base(viewer), body)
    },
    unset(viewer: ForwardViewer, address: string): Promise<ForwardChangeResult> {
      return daemonCliPost<ForwardChangeResult>(scope, `${base(viewer)}/unset`, { address })
    },
    pause(viewer: ForwardViewer, address: string): Promise<ForwardChangeResult> {
      return daemonCliPost<ForwardChangeResult>(scope, `${base(viewer)}/pause`, { address })
    },
    resume(viewer: ForwardViewer, address: string): Promise<ForwardChangeResult> {
      return daemonCliPost<ForwardChangeResult>(scope, `${base(viewer)}/resume`, { address })
    },
  }
}

export type ForwardApi = ReturnType<typeof forwardApiOn>

/** A staff-route refusal that means "try the person door": the route floor's
 *  codeless 403, or the forward routes' own forbidden / owner_only. */
export function isStaffRefusal(err: unknown): boolean {
  const { code } = mailErrorInfo(err)
  return code === undefined || code === 'forbidden' || code === 'owner_only'
}

/** May this viewer resume a forward paused for `reason`? Mirrors the
 *  daemon's `Caller::may_resume` so the panel only offers what will work
 *  (the daemon still decides). Staff here = owner or Admin. */
export function resumeRule(viewer: ForwardViewer, reason: string | null): { can: boolean; why: string | null } {
  if (reason === 'revoked') {
    return { can: false, why: 'The destination clicked Stop. Set the forward again to send a new confirm link.' }
  }
  if (reason === 'upgrade_required') {
    return { can: false, why: 'Not forwarding: the mail server needs upgrading (k2 hostmail upgrade). It resumes by itself after the upgrade.' }
  }
  if (reason === 'air_gap' || reason === 'receive_only') {
    return { can: false, why: 'K2 resumes it by itself when that changes.' }
  }
  if (reason === 'box_brake' && viewer === 'person') {
    return { can: false, why: 'Every outside forward on this server is paused. The owner or an Admin resumes them.' }
  }
  return { can: true, why: null }
}

/** One audit event in words. */
export function eventWords(e: ForwardEvent): string {
  const words: Record<string, string> = {
    set: 'Set',
    verify_sent: 'Confirm link sent',
    verified: 'Destination confirmed',
    activated: 'Forwarding on',
    paused: 'Paused',
    resumed: 'Resumed',
    removed: 'Removed',
    replaced: 'Replaced by a new forward',
    drift: 'Mail server repaired',
    imported: 'Imported',
    keep_changed: 'Keep a copy changed',
  }
  const base = words[e.event] ?? e.event
  const reason = e.detail && typeof e.detail.reason === 'string' ? e.detail.reason : null
  return reason ? `${base} (${reason.replace(/_/g, ' ')})` : base
}

/** The state chip's label. */
export function stateLabel(f: Forward): string {
  if (f.state === 'pending_verify') return f.verify?.state === 'expired' ? 'Link expired' : 'Waiting for confirm'
  if (f.state === 'active') return 'On'
  if (f.state === 'paused') return 'Paused'
  return f.state
}
