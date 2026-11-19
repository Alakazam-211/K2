// The Postal tab's rows (PostalTab.tsx): one peer per row with its status
// tag, its meta line, what the row means when that isn't obvious (a code
// check, a changed key, an invitation not sent yet), the asker's note, and
// the row's actions on the right. Plus the empty row and the line a card
// shows after an action in it.

import React from 'react'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL } from '../controls/SettingControls'
import { Tag, type TagTone } from '@/components/ui'
import { postalNotSentYet, postalRouteText, type PostalFriend, type PostalTime } from './postal-api'

/** A reading line's width in this tab: about 75 characters in any face. */
export const POSTAL_MEASURE = 'max-w-[var(--measure-text,75ch)]'
/** A row's hint at that width (SETTING_ROW_HINT's look). */
export const POSTAL_HINT = `type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed ${POSTAL_MEASURE}`

export function statusTag(friend: Pick<PostalFriend, 'status' | 'pairId'>): { text: string; tone: TagTone } | null {
  switch (friend.status) {
    case 'rekey_pending':
      return { text: 'Key changed', tone: 'warn' }
    case 'blocked':
      return { text: 'Blocked', tone: 'danger' }
    case 'pending_out':
      return postalNotSentYet(friend) ? { text: 'Not sent yet', tone: 'warn' } : { text: 'Waiting', tone: 'muted' }
    case 'pending_in':
      return { text: 'Asks to be friends', tone: 'accent' }
    case 'rejected':
      return { text: 'Rejected', tone: 'muted' }
    case 'revoked':
      return { text: 'Removed', tone: 'muted' }
    default:
      return null
  }
}

/** A daemon time as a date. The daemon sends unix seconds; a number past
 *  1e12 is read as milliseconds, and an ISO string is read as is. Anything
 *  else is ''. */
export function when(t: PostalTime | undefined): string {
  let d: Date
  if (typeof t === 'number' && Number.isFinite(t) && t > 0) d = new Date(t > 1e12 ? t : t * 1000)
  else if (typeof t === 'string' && t.trim()) d = new Date(t)
  else return ''
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
}

/** The address's handle status in words: `local` (kept here until the
 *  network confirms it), `claimed` (the network confirmed it), `released`. */
export function handleStatusText(status: string | null | undefined, createdAt: PostalTime | undefined): string {
  const since = when(createdAt)
  const head =
    status === 'local'
      ? 'Waiting for the Postal network to confirm it'
      : status === 'claimed'
        ? 'Confirmed on the Postal network'
        : status === 'released'
          ? 'Given up: other agents can’t reach it'
          : ''
  const claimed = since ? `claimed ${since}` : ''
  if (!head) return claimed ? `Claimed ${since}` : ''
  return [head, claimed].filter(Boolean).join(' · ')
}

/** Who the person compares codes with: the owner's name, else the peer's person. */
export function codePartner(friend: Pick<PostalFriend, 'ownerName' | 'peer'>): string {
  const owner = friend.ownerName?.trim()
  return owner ? owner : `${friend.peer}’s person`
}

/** What the row means, when its tag alone doesn't say. */
function rowExplainer(friend: PostalFriend): string | null {
  if (friend.status === 'pending_in' && friend.codeWaiting) {
    return `Code check: you’ll compare 6-digit codes with ${codePartner(friend)} before it’s accepted.`
  }
  if (friend.status === 'rekey_pending') {
    return `Its key changed on the Postal network, so its mail waits. Trust the new key once ${codePartner(friend)} says they changed it; otherwise remove it.`
  }
  if (postalNotSentYet(friend)) return 'It goes out once this server connects to the Postal network.'
  return null
}

export function InlineError({ message, testId }: { message: string | null; testId?: string }): React.JSX.Element | null {
  if (!message) return null
  return (
    <p role="alert" data-testid={testId} className={`type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed break-words ${POSTAL_MEASURE}`}>
      {message}
    </p>
  )
}

/** One peer in a list: its address, a status tag, its meta line, what the
 *  row means, the asker's note, and the row's actions on the right. `below`
 *  (an open form) goes full width under the lines. */
export function PeerRow({
  friend,
  actions,
  below,
  error,
  unblocked = false,
}: {
  friend: PostalFriend
  actions?: React.ReactNode
  below?: React.ReactNode
  error?: string | null
  /** A History row this person just unblocked (the daemon keeps it as
   *  removed): it says Unblocked. */
  unblocked?: boolean
}): React.JSX.Element {
  const tag = unblocked && friend.status === 'revoked' ? { text: 'Unblocked', tone: 'muted' as TagTone } : statusTag(friend)
  const lead = [friend.ownerName ? `Owner: ${friend.ownerName}` : '', postalRouteText(friend.route) ?? ''].filter(Boolean).join(' · ')
  // The date never breaks inside itself (a year alone on a line).
  const date = when(friend.decidedAt ?? friend.updatedAt ?? friend.createdAt)
  const meta =
    lead || date ? (
      <>
        {lead}
        {lead && date ? ' · ' : ''}
        {date ? <span className="whitespace-nowrap">{date}</span> : null}
      </>
    ) : null
  const explainer = rowExplainer(friend)
  const asking = friend.status === 'pending_in' || friend.status === 'pending_out'
  const note = asking ? friend.note?.trim() : ''
  return (
    <div className="px-4 py-3" data-testid="postal-peer-row" data-peer={friend.peer} data-status={friend.status}>
      <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
        {/* Narrow, the actions wrap under the address rather than squeeze it. */}
        <div className="min-w-0 grow basis-72">
          <div className={`${SETTING_ROW_LABEL} flex flex-wrap items-center gap-2 break-all`}>
            <span className="font-mono">{friend.peer}</span>
            {tag ? <Tag tone={tag.tone}>{tag.text}</Tag> : null}
          </div>
          {meta ? <div className={SETTING_ROW_HINT}>{meta}</div> : null}
        </div>
        {actions ? (
          <div className="flex flex-wrap items-center gap-2 flex-shrink-0" data-testid="postal-row-actions">
            {actions}
          </div>
        ) : null}
      </div>
      {/* What the row means and the note run the row's full width. */}
      {explainer ? (
        <div className={POSTAL_HINT} data-testid="postal-row-explainer">
          {explainer}
        </div>
      ) : null}
      {note ? (
        <p className={`${POSTAL_HINT} whitespace-pre-wrap break-words`} data-testid="postal-peer-note">
          {friend.status === 'pending_out' ? 'Your note: ' : 'Their note: '}“{note}”
        </p>
      ) : null}
      {below ? (
        <div className="mt-3" data-testid="postal-row-below">
          {below}
        </div>
      ) : null}
      <InlineError message={error ?? null} testId="postal-row-error" />
    </div>
  )
}

export function EmptyRow({ title, hint, testId }: { title: string; hint?: string; testId?: string }): React.JSX.Element {
  return (
    <div className="px-4 py-3" data-testid={testId}>
      <div className={SETTING_ROW_LABEL}>{title}</div>
      {hint ? <div className={`${POSTAL_HINT}`}>{hint}</div> : null}
    </div>
  )
}

/** What happened after an action, said in the card where it happened. */
export interface PostalNotice {
  text: string
  at: 'address' | 'friends' | 'invitations'
  tone: 'ok' | 'error'
}

export function CardNotice({ notice, at }: { notice: PostalNotice | null; at: PostalNotice['at'] }): React.JSX.Element | null {
  if (!notice || notice.at !== at) return null
  const error = notice.tone === 'error'
  return (
    <div className="px-4 py-3">
      <p
        role={error ? 'alert' : 'status'}
        data-testid="postal-notice"
        data-tone={notice.tone}
        className={`type-hint leading-relaxed break-words ${POSTAL_MEASURE} ${error ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-ok-soft)]'}`}
      >
        {notice.text}
      </p>
    </div>
  )
}
