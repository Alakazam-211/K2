// Who you are on the window's server, as its presence roster names you.
// Its own module (re-exported by PresenceAvatar) so a control can ask
// without loading the chip and its picture stores.

import { useConnectHostStore } from '@/stores/connect-host'

/** `owner` on this computer, else your saved login's username. */
export function useSelfPresenceUser(): string {
  return useConnectHostStore((s) => (s.activeHost === 'local' ? 'owner' : s.activeHost.username || 'owner'))
}
