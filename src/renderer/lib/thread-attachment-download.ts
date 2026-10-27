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
//
// 0.46.6 (prd-ticket-attachments-v1 TK19/TK22): a ticket's files save the
// same way through the ticket door (`ref.read`, `ticketAttachmentReader`),
// and a file SOMEONE ELSE attached (a Thread's or a ticket's) is saved with
// `quarantine: true`, so Gatekeeper asks before a dropped binary opens on
// macOS. Your own upload coming back doesn't nag.

import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { downloadChunks, type ReadRangeResponse } from '@/lib/fs-transfer'
import { readAttachment, type AttachmentReader } from '@/lib/thread-image-thumbs'
import type { ServerScope } from '@/kessel/server-scope'

/** The route's explicit-save mode. */
export const THREAD_DOWNLOAD_MODE = 'download'

/** Which attachment to save: the Thread's server and address, the
 *  message's doc id and the attachment's index on it (the daemon's `n`). */
export interface ThreadAttachmentDownloadRef {
  scope: ServerScope
  /** The Thread's address (the Thread door). */
  addr?: string
  /** Another door (a ticket's); wins over `addr`. */
  read?: AttachmentReader
  docId: string
  n: number
}

/** How a save lands. */
export interface ThreadAttachmentDownloadOptions {
  /** Someone else attached it: quarantine it (macOS). */
  fromOthers?: boolean
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
export function downloadThreadAttachment(
  ref: ThreadAttachmentDownloadRef,
  name: string,
  opts: ThreadAttachmentDownloadOptions = {},
): Promise<string | null> {
  return downloadChunks(
    name || 'file',
    (offset, len) => readAttachment(ref, { mode: THREAD_DOWNLOAD_MODE, offset, len }) as Promise<ReadRangeResponse>,
    opts.fromOthers ? { quarantine: true } : {},
  )
}
