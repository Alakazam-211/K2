// A server's name for a Home chat chip: "This computer", a saved server's
// label, else its host key.

import { LOCAL_HOME_HOST, savedHostForKey } from '@/lib/home-address'
import type { ConnectHost } from '@/stores/connect-host'

export function serverLabelFor(hostKey: string, hosts: readonly ConnectHost[]): string {
  if (hostKey === LOCAL_HOME_HOST) return 'This computer'
  return savedHostForKey(hosts as ConnectHost[], hostKey)?.label ?? hostKey
}
