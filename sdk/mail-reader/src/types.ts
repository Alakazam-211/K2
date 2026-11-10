// @k2/mail-reader types. Adapted from OCmail's lib/jmap/types.ts (MIT,
// Copyright (c) 2025 Matthieu MALVACHE, Copyright (c) 2026 Alakazam Labs; see
// ../NOTICE), trimmed to what K2's daemon serves on /cli/mail/reader/*. No type
// here carries a credential, a JMAP account id, a blob id or a server URL.

export interface ReaderAddress {
  name?: string | null
  address: string
}

/** SPF/DKIM/DMARC verdicts the daemon believes (null = no verdict). */
export interface AuthenticationResults {
  spf?: string | null
  dkim?: string | null
  dmarc?: string | null
}

/** Where an inbox lives. */
export type InboxSource = 'hosted' | 'linked'
/** Which service serves it. */
export type InboxProvider = 'hosted' | 'gmail' | 'microsoft' | 'imap'
/** An agent's inbox, or a person's own mailbox. */
export type InboxKind = 'agent' | 'person'

export interface ReaderInbox {
  address: string
  displayName?: string | null
  source: InboxSource
  provider: InboxProvider
  kind: InboxKind
  /** The viewer is this mailbox's own person. */
  own: boolean
  /** The workspace the inbox belongs to (its agent). */
  workspace?: string | null
  /** The person key the mailbox belongs to, when K2 knows it. */
  person?: string | null
  status?: string | null
}

export interface ReaderViewer {
  actor: string
  role: string
  /** Owner or Admin: may open every inbox (show "All inboxes"). */
  opensAny: boolean
}

/** A folder (OCmail's Mailbox, trimmed: no ids, rights or counts). */
export interface ReaderFolder {
  name: string
  role?: string | null
}

/** One message in a list (OCmail's Email, envelope only). */
export interface ReaderEmail {
  id: string
  inbox: string
  from?: ReaderAddress | null
  to?: ReaderAddress[]
  subject?: string | null
  date: string
  unread: boolean
  hasAttachment: boolean
  threadId?: string | null
}

export interface ReaderAttachment {
  /** 1-based, as the daemon numbers them. */
  index: number
  filename?: string | null
  mime: string
  size: number
}

/** One full message (person shape: no agent markers). */
export interface ReaderMessage extends ReaderEmail {
  cc?: ReaderAddress[]
  text?: string | null
  /** Already cleaned by the daemon; the views clean it again before framing. */
  html?: string | null
  remoteContentBlocked?: boolean
  attachments: ReaderAttachment[]
  authResults?: AuthenticationResults | null
}

/** Thread grouping for the list (OCmail's ThreadGroup). */
export interface ThreadGroup {
  threadId: string
  /** Newest first. */
  emails: ReaderEmail[]
  latestEmail: ReaderEmail
  participantNames: string[]
  hasUnread: boolean
  hasAttachment: boolean
  emailCount: number
}

export interface ListQuery {
  folder?: string
  offset?: number
  limit?: number
  query?: string
  from?: string
  since?: string
  before?: string
  unread?: boolean
}

export interface ListPage {
  items: ReaderEmail[]
  /** A number when there may be more. */
  nextOffset: number | null
}

/** An attachment's bytes, for a download (never opened inline). */
export interface ReaderAttachmentFile {
  filename: string
  mime: string
  size: number
  blob: Blob
}

export interface ReplyInput {
  id: string
  body: string
  all?: boolean
  reason?: string
}

export interface DraftInput {
  /** A reply draft names the message; a new draft names the inbox. */
  id?: string
  inbox?: string
  to?: string[]
  cc?: string[]
  subject?: string
  body: string
  reason?: string
}

export interface SendInput {
  inbox: string
  to: string[]
  cc?: string[]
  bcc?: string[]
  subject: string
  body: string
  reason?: string
}

export interface FlagInput {
  read?: boolean
  flagged?: boolean
}

/** A write's answer, as the daemon's k2 mail route gave it. */
export type MailWriteResult = Record<string, unknown>
