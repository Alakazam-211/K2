// The `/cli/desktop/*` routes (prd-remote-desktop-tab-v1 §5) on a scope's
// server. Thin: the daemon decides who may do what; these only send.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import type { DesktopGrant, DesktopStatus, DesktopViewer } from './desktop-state'
import type { Display } from './desktop-wire'

export interface DesktopAuditLine {
  ts: string
  event: string
  who: string
  role?: string
  ingress?: string
  ip?: string
  device?: string
  display?: string
  target?: string
  mode?: string
  durationMs?: number
  outcome?: string
}

export function fetchDesktopStatus(scope: ServerScope): Promise<DesktopStatus> {
  return daemonCliGet<DesktopStatus>(scope, 'desktop/status')
}

export async function fetchDesktopDisplays(scope: ServerScope): Promise<Display[]> {
  const r = await daemonCliGet<{ displays: Display[] }>(scope, 'desktop/displays')
  return r.displays
}

export function enableDesktop(scope: ServerScope): Promise<{ enabled: boolean; state: string; card?: string }> {
  return daemonCliPost(scope, 'desktop/enable', {})
}

export function disableDesktop(scope: ServerScope): Promise<{ enabled: boolean; closed: number }> {
  return daemonCliPost(scope, 'desktop/disable', {})
}

export function setDesktopHideApps(scope: ServerScope, hideApps: string[]): Promise<{ hideApps: string[] }> {
  return daemonCliPost(scope, 'desktop/policy', { hideApps })
}

export function grantDesktop(scope: ServerScope, user: string): Promise<{ grant: DesktopGrant }> {
  return daemonCliPost(scope, 'desktop/grant', { user, mode: 'view' })
}

export function revokeDesktop(scope: ServerScope, user: string): Promise<{ revoked: boolean; closed: number }> {
  return daemonCliPost(scope, 'desktop/revoke', { user })
}

export async function fetchDesktopGrants(scope: ServerScope): Promise<DesktopGrant[]> {
  const r = await daemonCliGet<{ grants: DesktopGrant[] }>(scope, 'desktop/grants')
  return r.grants
}

export async function fetchDesktopViewers(scope: ServerScope): Promise<DesktopViewer[]> {
  const r = await daemonCliGet<{ viewers: DesktopViewer[] }>(scope, 'desktop/viewers')
  return r.viewers
}

export function kickDesktopViewer(scope: ServerScope, viewer: string): Promise<{ kicked: number }> {
  return daemonCliPost(scope, 'desktop/kick', { viewer })
}

export async function fetchDesktopAudit(scope: ServerScope, limit = 50): Promise<DesktopAuditLine[]> {
  const r = await daemonCliGet<{ lines: DesktopAuditLine[] }>(scope, 'desktop/audit', { limit })
  return r.lines
}
