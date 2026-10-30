// The Live view's transport bound to a server scope: the daemon's creds for
// that server, `WS /cli/docs/live`, dialled through the server's socket
// queue (MS70) like every other daemon socket. The transport itself is
// `docs-live-transport.ts`.

import { getDaemonWs, daemonWsBase } from '@/kessel/daemon-ws'
import type { ServerScope } from '@/kessel/server-scope'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { createLiveTransport, liveUrl, type LiveSocket, type LiveTransport } from './docs-live-transport'

/** A Live transport for `path` on `scope`'s server (`path` absolute, or
 *  workspace-relative with `workspace`). */
export function createDaemonLiveTransport(scope: ServerScope, doc: { path: string; workspace?: string }): LiveTransport {
  return createLiveTransport({
    url: async (resume) => {
      const creds = await getDaemonWs(scope)
      return liveUrl(daemonWsBase(creds), creds.token, doc.path, { workspace: doc.workspace, resume })
    },
    open: async (url, signal) => (await openQueuedWebSocket(scope, url, { signal })) as unknown as LiveSocket,
  })
}
