// Thread push (prd-thread-push-notifications-v1 S5): the caller's own push
// prefs on one server (the window's, unless a room passes its scope), kept
// live. Shared by Settings → Notifications, the workspace Agent tab's phone
// switch and the Workspace drawer's phone toggle, so all show the daemon's
// one truth and refetch on `notify_prefs_changed`.

import { useCallback, useEffect, useRef, useState } from 'react'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onNotifyPrefsChanged } from '@/stores/session-events'
import { fetchNotifyPrefs, isYouAuthor, type NotifyPrefs } from './notify-api'

/** Does the window's server have Thread push (`thread-push-v1` in its
 *  `/boot-status` features)? Without it, every Thread push surface hides. */
export function threadPushAvailable(): boolean {
  return primaryScope().serverSupports('thread-push-v1')
}

export interface NotifyPrefsState {
  prefs: NotifyPrefs | null
  error: string | null
  /** Re-read now. */
  reload: () => void
  /** Adopt a write's answer (the POST returns the same shape as the GET). */
  adopt: (next: NotifyPrefs) => void
}

/** `enabled` false: nothing is fetched or subscribed (an older server).
 *  `scope`: the server to read (a room's drawer passes `room.scope`). */
export function useNotifyPrefs(enabled: boolean, scope: ServerScope = primaryScope()): NotifyPrefsState {
  const [prefs, setPrefs] = useState<NotifyPrefs | null>(null)
  const [error, setError] = useState<string | null>(null)
  /** Only the newest read lands (a refetch can overtake an older one). */
  const generation = useRef(0)

  const reload = useCallback(() => {
    if (!enabled) return
    const gen = ++generation.current
    fetchNotifyPrefs(scope).then(
      (p) => {
        if (gen !== generation.current) return
        setPrefs(p)
        setError(null)
      },
      (e: unknown) => {
        if (gen !== generation.current) return
        setError(e instanceof Error ? e.message : String(e))
      },
    )
  }, [enabled, scope])

  useEffect(() => {
    if (!enabled) {
      setPrefs(null)
      setError(null)
      return
    }
    reload()
    const off = onNotifyPrefsChanged(scope, reload)
    return () => {
      off()
      generation.current += 1
    }
  }, [enabled, scope, reload])

  const adopt = useCallback((next: NotifyPrefs) => {
    generation.current += 1
    setPrefs(next)
    setError(null)
  }, [])

  return { prefs, error, reload, adopt }
}

/** "You" for a chat/ticket author stamp on the window's server: `owner`,
 *  plus — when this viewer is the owner person (the owner token or a login
 *  marked "This is me (owner)") — every login linked to the owner. */
export function useIsYouAuthor(): (author: string) => boolean {
  const { prefs } = useNotifyPrefs(threadPushAvailable())
  return useCallback((author: string) => isYouAuthor(author, prefs), [prefs])
}
