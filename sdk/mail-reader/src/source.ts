// The one data seam (prd-ocmail-admin-in-k2-v1 §5.3). The views never talk to
// a server; the host hands them a MailReaderSource. K2's renderer implements
// it over the daemon's /cli/mail/reader/* routes; the standalone App (people
// signed in as their App user) brings its own.
//
// Reads never change the mailbox: `message` and `thread` are peeks (nothing is
// marked read). Every write is an explicit user action in the views.

import type {
  DraftInput,
  FlagInput,
  ListPage,
  ListQuery,
  MailWriteResult,
  ReaderAttachmentFile,
  ReaderFolder,
  ReaderInbox,
  ReaderMessage,
  ReaderViewer,
  ReplyInput,
  SendInput,
} from './types'

export interface MailReaderSource {
  /** The inboxes this viewer may open, and who the viewer is. */
  inboxes(): Promise<{ viewer: ReaderViewer; inboxes: ReaderInbox[] }>
  folders(inbox: string): Promise<ReaderFolder[]>
  list(inbox: string, q: ListQuery): Promise<ListPage>
  /** One message. Never marks it read. */
  message(id: string): Promise<ReaderMessage>
  /** The messages of a message's thread, oldest first. Never marks read. */
  thread(id: string, folder?: string): Promise<ReaderMessage[]>
  /** An attachment's bytes (1-based part), for a download. */
  attachment(id: string, part: number): Promise<ReaderAttachmentFile>

  // ── Writes (explicit user actions only) ──
  move(id: string, folder: string): Promise<MailWriteResult>
  /** The only way the reader changes read/unread or the star. */
  flag(id: string, flags: FlagInput): Promise<MailWriteResult>
  archive(id: string): Promise<MailWriteResult>
  /** Moves to Trash; never a permanent delete. */
  delete(id: string): Promise<MailWriteResult>
  reply(input: ReplyInput): Promise<MailWriteResult>
  draft(input: DraftInput): Promise<MailWriteResult>
  send(input: SendInput): Promise<MailWriteResult>
}

/** The names of the write methods (the views call them only from buttons). */
export const WRITE_METHODS = ['move', 'flag', 'archive', 'delete', 'reply', 'draft', 'send'] as const satisfies readonly (keyof MailReaderSource)[]
