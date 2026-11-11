// Whether the top bar shows the Mail tab (OA5): for an Owner, Admin or Member
// login (and the owner token) on the window's server, and only when that
// server has mail: hosted mail enabled or any linked inbox (Rosson
// 2026-11-10: "The mail icon should only pop up when/if there is a linked
// inbox or if there is mail hosting enabled"). The daemon answers that from
// its own state (GET /cli/mail/reader/presence); a Member on a server with
// mail but no mailbox of their own still sees the tab and the reader's
// "no mailboxes" line.
//
// A Voice & Chat only login never sees it (whoami answers `voice_chat`,
// which fetchViewerRole reports as no role; such a login also gets the
// Voice & Chat gate instead of the app). The daemon refuses the reader
// routes below Member either way.
//
// Live: re-checked on every `mail_changed` from the server (hosting turned
// on or off arrives as `server-state-changed`, a linked inbox added or
// removed as `inboxes-changed`), and on window focus as a fallback.

import { useEffect, useState } from 'react'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { useConnectHostStore } from '@/stores/connect-host'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { onMailChanged } from '@/stores/session-events'

/** Route, after `/cli/`. */
export const MAIL_PRESENCE_ROUTE = 'mail/reader/presence'

/** What the daemon says about mail on its server. */
export interface MailPresence {
  /** Hosted mail is enabled on this box. */
  hosting: boolean
  /** Any linked inbox exists on the server. */
  linkedInboxes: boolean
  /** How many inboxes this viewer could open in the reader. */
  visible: number
}

/** The roles that may use the mail reader. */
export function mailRoleAllows(role: string | null): boolean {
  return role === 'owner' || role === 'admin' || role === 'member'
}

/** The server has mail: hosting on, or a linked inbox. */
export function serverHasMail(p: MailPresence | null): boolean {
  return p !== null && (p.hosting || p.linkedInboxes)
}

export function mailTabShownFor(role: string | null, presence: MailPresence | null): boolean {
  return mailRoleAllows(role) && serverHasMail(presence)
}

/** The window's server's answer, or null when it gave none (an older
 *  daemon without the route, or no connection): the tab stays hidden. */
export async function fetchMailPresence(): Promise<MailPresence | null> {
  try {
    const r = await daemonCliGet<Partial<MailPresence>>(primaryScope(), MAIL_PRESENCE_ROUTE)
    if (typeof r?.hosting !== 'boolean' || typeof r.linkedInboxes !== 'boolean') return null
    return { hosting: r.hosting, linkedInboxes: r.linkedInboxes, visible: typeof r.visible === 'number' ? r.visible : 0 }
  } catch {
    return null
  }
}

// The last answer per server, so a top bar mounting again (a page switch)
// starts from it instead of hiding the tab until the fetch lands.
const lastShown = new Map<string, boolean>()

/** Test-only: forget the remembered answers. */
export function __resetMailTabForTests(): void {
  lastShown.clear()
}

export function useMailTabShown(): boolean {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [shown, setShown] = useState(() => lastShown.get(hostKey) ?? false)
  useEffect(() => {
    let gone = false
    let seq = 0
    setShown(lastShown.get(hostKey) ?? false)
    const check = async (): Promise<void> => {
      const mine = ++seq
      const role = await fetchViewerRole()
      const next = mailRoleAllows(role) ? serverHasMail(await fetchMailPresence()) : false
      if (gone || mine !== seq) return
      lastShown.set(hostKey, next)
      setShown(next)
    }
    void check()
    const offMail = onMailChanged(primaryScope(), () => void check())
    const onFocus = (): void => void check()
    window.addEventListener('focus', onFocus)
    return () => {
      gone = true
      offMail()
      window.removeEventListener('focus', onFocus)
    }
  }, [hostKey])
  return shown
}
