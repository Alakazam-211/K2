// A URL clicked in a terminal (Home M5).
//
// The window's own room asks its daemon to open it (`fs/open-external`),
// as before. A room on another server must not: that route opens a browser
// on THAT server's machine, and it is not on the room write allowlist
// (`kessel/room-writes.ts`). The person clicking is at this computer, so a
// remote room opens the link here.

import { daemonCliPost } from '@/lib/daemon-cli'
import { scopeMayWrite, type ServerScope } from '@/kessel/server-scope'

export function openTerminalUrl(room: { isPrimary: boolean; scope: ServerScope }, url: string): void {
  if (!room.isPrimary && !scopeMayWrite(room.scope, 'fs/open-external')) {
    void import('@tauri-apps/plugin-opener')
      .then(({ openUrl }) => openUrl(url))
      .catch((err: unknown) => console.warn('[terminal-link] open here failed', err))
    return
  }
  daemonCliPost(room.scope, 'fs/open-external', { target: url }).catch((err: unknown) =>
    console.warn('[terminal-link]', err),
  )
}
