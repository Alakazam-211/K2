// Download a Thread file (0.46.3, Rosson 2026-10-22: "a download button on
// the right hand side" of every file tile in a Thread).
//
// The daemon decides who may save (daemon-first): each attachment it sends
// carries `canDownload` for this viewer (people: true; an App guest only
// with `files:download`), and it enforces the same rule on the route. The
// client draws the button only when the daemon said true, and a click
// reads the file through the same route that opens it today,
// `GET /cli/thread/attachment`, with `mode=download` (the `range` chunks
// plus `name` and `mime`), into the shared download lifecycle
// (`lib/fs-transfer`: transfer card, Cancel, toasts; `~/Downloads` on the
// desktop, a browser save on the web client).

import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { daemonCliGet } from '@/lib/daemon-cli'
import { downloadChunks, type ReadRangeResponse } from '@/lib/fs-transfer'
import { THREAD_ATTACHMENT_ROUTE } from '@/lib/thread-image-thumbs'
import type { ServerScope } from '@/kessel/server-scope'

/** The route's explicit-save mode. */
export const THREAD_DOWNLOAD_MODE = 'download'

/** Which attachment to save: the Thread's server and address, the
 *  message's doc id and the attachment's index on it (the daemon's `n`). */
export interface ThreadAttachmentDownloadRef {
  scope: ServerScope
  addr: string
  docId: string
  n: number
}

/** Draw a Download button for `f`? Only when the daemon said so, and only
 *  for a file this viewer can see that is still there. */
export function threadFileDownloadable(f: ThreadFileObject): boolean {
  if (f.canDownload !== true) return false
  if (f.state === 'hidden' || f.state === 'gone') return false
  return !!f.path && !!f.name
}

/** Save the attachment through the daemon. Resolves to the saved path or
 *  name, or null (failed or cancelled; the toast already said so). */
export function downloadThreadAttachment(ref: ThreadAttachmentDownloadRef, name: string): Promise<string | null> {
  return downloadChunks(name || 'file', (offset, len) =>
    daemonCliGet<ReadRangeResponse>(ref.scope, THREAD_ATTACHMENT_ROUTE, {
      addr: ref.addr,
      id: ref.docId,
      n: ref.n,
      mode: THREAD_DOWNLOAD_MODE,
      offset,
      len,
    }),
  )
}
