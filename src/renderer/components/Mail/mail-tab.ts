// Whether the top bar shows the Mail tab (OA5): for an Owner, Admin or Member
// login (and the owner token) on the window's server. A Voice & Chat only
// login never sees it (whoami answers `voice_chat`, which fetchViewerRole
// reports as no role; such a login also gets the Voice & Chat gate instead of
// the app). The daemon refuses the reader routes below Member either way.

import { useEffect, useState } from 'react'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { useConnectHostStore } from '@/stores/connect-host'

export function mailTabShownFor(role: string | null): boolean {
  return role === 'owner' || role === 'admin' || role === 'member'
}

export function useMailTabShown(): boolean {
  const hostKey = useConnectHostStore((s) => (s.activeHost === 'local' ? 'local' : s.activeHost.id))
  const [shown, setShown] = useState(false)
  useEffect(() => {
    let gone = false
    void fetchViewerRole().then((r) => {
      if (!gone) setShown(mailTabShownFor(r))
    })
    return () => {
      gone = true
    }
  }, [hostKey])
  return shown
}
