// The connection-light colours a server icon's LEDs use, shared by the
// server switcher and the top bar's centre breadcrumb (kept apart from
// ServerSwitcher.tsx so the title does not load the switcher).

import type { ConnectionStatus } from '@/stores/connect-host'

export function statusColor(status: ConnectionStatus): string {
  switch (status) {
    case 'connected':
      return '#3fb950' // green
    case 'connecting':
      return '#d29922' // amber
    case 'offline':
      return '#f85149' // red
  }
}
