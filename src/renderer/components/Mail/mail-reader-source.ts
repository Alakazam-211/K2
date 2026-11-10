// The renderer's MailReaderSource (prd-ocmail-admin-in-k2-v1 OA5): every call
// goes to the daemon's person-facing reader routes, /cli/mail/reader/*, on the
// window's server. Never /cli/mail/read (that one marks mail read for the
// mailbox's own reader) and never project=: the daemon takes the viewer from
// the login and decides who may open what, logs it and refuses agents.

import type {
  DraftInput,
  FlagInput,
  ListPage,
  ListQuery,
  MailReaderSource,
  MailWriteResult,
  ReaderAttachmentFile,
  ReaderFolder,
  ReaderInbox,
  ReaderMessage,
  ReaderViewer,
  ReplyInput,
  SendInput,
} from '@k2/mail-reader'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

/** Route prefix, after `/cli/`. */
export const READER_ROUTE = 'mail/reader'

interface InboxesAnswer {
  viewer?: ReaderViewer
  inboxes?: ReaderInbox[]
}
interface FoldersAnswer {
  folders?: ReaderFolder[]
}
interface MessagesAnswer {
  items?: ReaderMessage[]
  nextOffset?: number | null
}
interface MessageAnswer {
  message?: ReaderMessage
}
interface ThreadAnswer {
  messages?: ReaderMessage[]
}
interface AttachmentAnswer {
  filename?: string
  mime?: string
  size?: number
  dataBase64?: string
}

function requireField<T>(value: T | undefined | null, what: string): T {
  if (value === undefined || value === null) throw new Error(`the mail reader answered without ${what}`)
  return value
}

function bytesFromBase64(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

function listParams(inbox: string, q: ListQuery): Record<string, string | number | undefined> {
  return {
    inbox,
    folder: q.folder,
    offset: q.offset,
    limit: q.limit,
    query: q.query,
    from: q.from,
    since: q.since,
    before: q.before,
    unread: q.unread ? 1 : undefined,
  }
}

export function daemonMailReaderSource(scope: ServerScope): MailReaderSource {
  const get = <T>(route: string, params?: Record<string, string | number | undefined>): Promise<T> =>
    daemonCliGet<T>(scope, `${READER_ROUTE}/${route}`, params)
  const post = (route: string, body: object): Promise<MailWriteResult> =>
    daemonCliPost<MailWriteResult>(scope, `${READER_ROUTE}/${route}`, body).then((r) => r ?? {})

  return {
    async inboxes() {
      const r = await get<InboxesAnswer>('inboxes')
      return {
        viewer: requireField(r.viewer, 'the viewer'),
        inboxes: requireField(r.inboxes, 'the inbox list'),
      }
    },
    async folders(inbox) {
      const r = await get<FoldersAnswer>('folders', { inbox })
      return requireField(r.folders, 'the folder list')
    },
    async list(inbox, q): Promise<ListPage> {
      const r = await get<MessagesAnswer>('messages', listParams(inbox, q))
      return {
        items: requireField(r.items, 'the message list'),
        nextOffset: typeof r.nextOffset === 'number' ? r.nextOffset : null,
      }
    },
    async message(id) {
      const r = await get<MessageAnswer>('message', { id })
      return requireField(r.message, 'the message')
    },
    async thread(id, folder) {
      const r = await get<ThreadAnswer>('thread', { id, folder })
      return requireField(r.messages, 'the thread')
    },
    async attachment(id, part): Promise<ReaderAttachmentFile> {
      const r = await get<AttachmentAnswer>('attachment', { id, part })
      const mime = r.mime || 'application/octet-stream'
      const bytes = bytesFromBase64(requireField(r.dataBase64, 'the attachment'))
      return {
        filename: r.filename || `attachment-${part}`,
        mime,
        size: typeof r.size === 'number' ? r.size : bytes.length,
        blob: new Blob([bytes as BlobPart], { type: mime }),
      }
    },
    move: (id: string, folder: string) => post('move', { id, folder }),
    flag: (id: string, flags: FlagInput) => post('flag', { id, ...flags }),
    archive: (id: string) => post('archive', { id }),
    delete: (id: string) => post('delete', { id }),
    reply: (input: ReplyInput) => post('reply', input),
    draft: (input: DraftInput) => post('draft', input),
    send: (input: SendInput) => post('send', input),
  }
}
