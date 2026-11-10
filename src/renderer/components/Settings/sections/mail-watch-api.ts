// Agent mail notices: bindings for the daemon's `/cli/mail/watch*` routes.
// One record, two screens (an agent's Mail tab and an inbox's own panel): both
// read and write here, so they can't disagree.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type InboxWake = 'vip' | 'all'

/** One inbox this agent can read, and whether it wakes it. */
export interface WatchInbox {
  address: string
  source: 'hosted' | 'linked'
  /** Null = off: this inbox never wakes the agent. */
  wake: InboxWake | null
  primary: boolean
  personMailbox: boolean
  lastPollAt: string | null
  lastError: string | null
  needsRelink: boolean
}

export interface WatchDigest {
  everySeconds: number | null
  start: string | null
  end: string | null
  days: string[] | null
  tz: string | null
  nextFireAt: string | null
  enabled: boolean
}

export interface MailWatchStatus {
  enabled: boolean
  vipOutsideWindow: boolean
  snippets: boolean
  vipSessionLine: boolean
  pausedUntil: string | null
  inboxes: WatchInbox[]
  vips: string[]
  digest: WatchDigest | null
  last24h: { vipDelivered: number; digested: number; failedAuth: number }
}

/** One agent that hears about an inbox. */
export interface InboxWatcher {
  workspace: string
  projectId: string
  wake: InboxWake
  watching: boolean
}

export function fetchMailWatch(project: string): Promise<MailWatchStatus> {
  return daemonCliGet<MailWatchStatus>(primaryScope(), 'mail/watch', { project })
}

export function fetchInboxWatchers(address: string): Promise<{ agents: InboxWatcher[] }> {
  return daemonCliGet<{ agents: InboxWatcher[] }>(primaryScope(), 'mail/watch/inbox', { address })
}

export interface WatchSetBody {
  on?: boolean
  vipOutsideWindow?: boolean
  snippets?: boolean
  vipSessionLine?: boolean
  digestEverySecs?: number
  window?: { start?: string; end?: string; days?: string[]; tz?: string; off?: boolean }
}

export function setMailWatch(project: string, body: WatchSetBody): Promise<MailWatchStatus> {
  return daemonCliPost<MailWatchStatus>(primaryScope(), 'mail/watch/set', { project, ...body })
}

/** Choose how one inbox wakes this agent. `null` turns it off. */
export function setWatchInbox(project: string, address: string, wake: InboxWake | null): Promise<MailWatchStatus> {
  const body = wake ? { project, add: [{ address, wake }] } : { project, remove: [address] }
  return daemonCliPost<MailWatchStatus>(primaryScope(), 'mail/watch/inboxes', body)
}

export function addWatchVip(project: string, entry: string): Promise<MailWatchStatus> {
  return daemonCliPost<MailWatchStatus>(primaryScope(), 'mail/watch/vip', { project, add: [entry] })
}

export function removeWatchVip(project: string, entry: string): Promise<MailWatchStatus> {
  return daemonCliPost<MailWatchStatus>(primaryScope(), 'mail/watch/vip', { project, remove: [entry] })
}

export function pauseMailWatch(project: string, resume: boolean, duration?: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'mail/watch/pause', { project, resume, ...(duration ? { for: duration } : {}) })
}

export const WAKE_OPTIONS: { value: string; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'vip', label: 'VIP senders only' },
  { value: 'all', label: 'VIPs now, others in a digest' },
]

export function wakeValue(w: InboxWake | null): string {
  return w ?? 'off'
}

export function wakeFromValue(value: string): InboxWake | null {
  return value === 'vip' || value === 'all' ? value : null
}

export const WEEKDAYS: { value: string; label: string }[] = [
  { value: 'mon', label: 'Mon' },
  { value: 'tue', label: 'Tue' },
  { value: 'wed', label: 'Wed' },
  { value: 'thu', label: 'Thu' },
  { value: 'fri', label: 'Fri' },
  { value: 'sat', label: 'Sat' },
  { value: 'sun', label: 'Sun' },
]
