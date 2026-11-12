// The `/cli/desktop/*` routes (prd-remote-desktop-tab-v1 §5) on a scope's
// server. Thin: the daemon decides who may do what; these only send.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { readDesktopList, type DesktopGrant, type DesktopList, type DesktopStatus, type DesktopViewer } from './desktop-state'
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

/** `desktop` names the desktop (prd-linux-virtual-desktop-v1 §3.1); left
 *  out, This server. */
export function fetchDesktopStatus(scope: ServerScope, desktop?: string): Promise<DesktopStatus> {
  return daemonCliGet<DesktopStatus>(scope, 'desktop/status', desktop ? { desktop } : undefined)
}

/** `GET /cli/desktop/list`: the desktops this person may open.
 *  `workspace`: the project id a new tab opens in; the reply's `default` is
 *  that workspace's agent desktop when its switch is on, else This server. */
export async function fetchDesktopList(scope: ServerScope, workspace?: string | null): Promise<DesktopList> {
  return readDesktopList(await daemonCliGet<unknown>(scope, 'desktop/list', workspace ? { workspace } : undefined))
}

/** Start (or wake) a virtual desktop at this pane's size (§3.5). */
export function startDesktop(scope: ServerScope, desktop: string, size?: { width: number; height: number }): Promise<unknown> {
  return daemonCliPost(scope, 'desktop/start', { desktop, ...(size ? { width: size.width, height: size.height } : {}) })
}

/** Close a virtual desktop: what was open on it is lost. */
export function stopDesktop(scope: ServerScope, desktop: string): Promise<unknown> {
  return daemonCliPost(scope, 'desktop/stop', { desktop })
}

/** Install the desktop packages through the root helper (an Owner's yes). */
export function setupDesktop(scope: ServerScope): Promise<unknown> {
  return daemonCliPost(scope, 'desktop/setup', {})
}

export async function fetchDesktopDisplays(scope: ServerScope): Promise<Display[]> {
  const r = await daemonCliGet<{ displays: Display[] }>(scope, 'desktop/displays')
  return r.displays
}

/** `desktop`: the switch to turn on (`virtual`: every virtual desktop on
 *  the server, §4 item 2); left out, This server's. */
export function enableDesktop(scope: ServerScope, desktop?: string): Promise<{ enabled: boolean; state: string; card?: string }> {
  return daemonCliPost(scope, 'desktop/enable', desktop ? { desktop } : {})
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
