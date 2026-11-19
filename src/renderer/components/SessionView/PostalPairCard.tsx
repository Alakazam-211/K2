// The Postal friend request card's own parts (`postal.pair.accept`,
// 0.46.16), for PermissionCard: the asker's note on its own line in place of
// the raw request chips (workspace, id, pairing), the tab's words on the
// buttons (Accept or "Type their code", and Reject), and K2's own code form
// on the card.
//
// The code form shows this server's code (for the person to read out) and
// posts the code from the OTHER side's screen straight to the decide route
// as this person; only then is Allow sent. Neither code passes through the
// card, the Thread or the agent. The friend row (peer, owner, note) is read
// as the person who may answer, from the same route the Postal tab reads;
// it never carries a code.

import { useEffect, useState, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { PostalPairCodeForm, readCodeTo } from '@/components/Settings/sections/PostalPairCodeForm'
import { fetchPostalFriends, type PostalFriend } from '@/components/Settings/sections/postal-api'
import { CARD_MUTED, CARD_TEXT2, CardButton } from './cardControls'
import type { PermissionItem } from './permissionCardView'

/** A Postal friend request; `pairing=code` when it carries a pairing code. */
export const POSTAL_PAIR_ACCEPT_ACTION = 'postal.pair.accept'

/** The tab's words for the card's answers (the answers sent stay Allow and Deny). */
export const POSTAL_PAIR_LABEL = { allow: 'Accept', code: 'Type their code', deny: 'Reject' } as const

export type PostalPairFriend = Pick<PostalFriend, 'peer' | 'ownerName' | 'note'>

function valueOf(item: PermissionItem, key: string): string {
  return item.values.find((v) => v.key === key)?.value ?? ''
}

/** Does this card's request carry a pairing code? */
export function postalPairWantsCode(item: PermissionItem): boolean {
  return item.action === POSTAL_PAIR_ACCEPT_ACTION && valueOf(item, 'pairing') === 'code'
}

/** The request's friend row, read as the person who may answer; null until
 *  it's read, and for any other card. */
export function usePostalPairFriend(item: PermissionItem, scope: ServerScope | null | undefined): PostalPairFriend | null {
  const postal = item.action === POSTAL_PAIR_ACCEPT_ACTION
  const id = postal ? valueOf(item, 'id') : ''
  const workspace = postal ? valueOf(item, 'workspace') : ''
  const [friend, setFriend] = useState<PostalPairFriend | null>(null)
  useEffect(() => {
    if (!scope || !id || !workspace) return
    let live = true
    fetchPostalFriends(scope, workspace)
      .then((v) => {
        const f = Array.isArray(v.friends) ? v.friends.find((x) => x.id === id) : undefined
        if (live && f) setFriend({ peer: f.peer, ownerName: f.ownerName ?? null, note: f.note ?? null })
      })
      .catch(() => {
        // The card's own words still say who asked; only the note waits.
      })
    return () => {
      live = false
    }
  }, [scope, id, workspace])
  return friend
}

/** The daemon puts the asker's note last in the card's words
 *  (` Their note: “…”`); a card that shows the note on its own line drops it
 *  there. */
export function withoutPostalNote(consequences: string): string {
  return consequences.replace(/\s*Their note: (“[\s\S]*”|"[\s\S]*"\.?)\s*$/, '')
}

/** The asker's note, in quotes, on its own line. */
export function PostalPairNote({ friend }: { friend: PostalPairFriend | null }): JSX.Element | null {
  const note = friend?.note?.trim()
  if (!note) return null
  return (
    <div data-testid="postal-pair-note" className="whitespace-pre-wrap break-words" style={{ color: CARD_TEXT2 }}>
      <span style={{ color: CARD_MUTED }}>Their note: </span>“{note}”
    </div>
  )
}

/** "Type their code" opens K2's code form; Reject opens the card's own
 *  reason step (`onDeny`). Allow goes once the daemon took the code. */
export function PostalPairCodeAnswer({
  item,
  scope,
  friend,
  busy,
  onAllow,
  onDeny,
}: {
  item: PermissionItem
  scope: ServerScope
  friend: PostalPairFriend | null
  busy: boolean
  onAllow: () => void
  onDeny: () => void
}): JSX.Element {
  const [coding, setCoding] = useState(false)
  const peer = friend?.peer ?? 'the other agent'
  if (coding) {
    return (
      <PostalPairCodeForm
        scope={scope}
        variant="card"
        workspace={valueOf(item, 'workspace')}
        requestId={valueOf(item, 'id')}
        peer={peer}
        owner={friend?.ownerName ?? null}
        onDone={onAllow}
        onCancel={() => setCoding(false)}
      />
    )
  }
  return (
    <>
      <div className="flex flex-wrap gap-1.5">
        <CardButton label={POSTAL_PAIR_LABEL.code} testId="permission-allow" primary disabled={busy} onClick={() => setCoding(true)} />
        <CardButton label={POSTAL_PAIR_LABEL.deny} testId="permission-deny" disabled={busy} onClick={onDeny} />
      </div>
      <div data-testid="permission-code-note" className="text-[10px] leading-relaxed" style={{ color: CARD_MUTED }}>
        You read your code to {friend ? readCodeTo(friend.peer, friend.ownerName) : 'the other side'} and type theirs here. The agent never sees either code.
      </div>
    </>
  )
}
