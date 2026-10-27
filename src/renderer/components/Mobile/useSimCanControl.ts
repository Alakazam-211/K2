// May this viewer tap, type, boot and stop the device? (Rosson 2026-10-27:
// remote Connect Members watch, they don't tap.)
//
// The daemon decides and enforces it. The tab only hides controls it would
// refuse:
//  - the status's `canControl`, when the daemon sends one, is the answer;
//  - otherwise: this computer's daemon → yes; a remote server → ask
//    `auth/whoami` once: Owner or Admin → yes, Member (or no answer) → no;
//  - a Home room on another server also needs the sim routes on its room
//    write allowlist (`kessel/room-writes.ts`).

import { useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'
import { SIM_BOOT_ROUTE } from './simApi'

export type ViewerRole = 'owner' | 'admin' | 'member'

/** Pure: the rule above. `role` null = not known (yet). */
export function simControlFor(args: {
  daemonSays: boolean | null
  isRemote: boolean
  role: ViewerRole | null
  roomMayWrite: boolean
}): boolean {
  if (!args.roomMayWrite) return false
  if (args.daemonSays !== null) return args.daemonSays
  if (!args.isRemote) return true
  return args.role === 'owner' || args.role === 'admin'
}

function asRole(v: unknown): ViewerRole | null {
  return v === 'owner' || v === 'admin' || v === 'member' ? v : null
}

export function useSimCanControl(scope: ServerScope, daemonSays: boolean | null): boolean {
  const [role, setRole] = useState<ViewerRole | null>(null)
  const ask = daemonSays === null && scope.isRemote
  useEffect(() => {
    if (!ask) return
    let cancelled = false
    daemonCliGet<{ owner?: unknown; role?: unknown }>(scope, 'auth/whoami')
      .then((w) => {
        if (!cancelled) setRole(asRole(w.role) ?? (w.owner === true ? 'owner' : null))
      })
      .catch((err: unknown) => console.warn('[mobile] auth/whoami:', err))
    return () => {
      cancelled = true
    }
  }, [scope, ask])
  return simControlFor({ daemonSays, isRemote: scope.isRemote, role, roomMayWrite: scopeMayWrite(scope, SIM_BOOT_ROUTE) })
}
