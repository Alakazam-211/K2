// @k2/mail-reader: K2's mail reader views behind one data seam
// (prd-ocmail-admin-in-k2-v1 OA4). Parts are adapted from OCmail (MIT; see
// ../NOTICE and ../UPSTREAM.md). The host supplies a MailReaderSource (data)
// and renderHtml (its own sandboxed frame); the package makes no network
// calls and creates no frames.

export type { MailReaderSource } from './source'
export { WRITE_METHODS } from './source'
export type * from './types'
export { t, type MessageKey } from './i18n'
export {
  displayAddress,
  folderLabel,
  folderOrder,
  folderParam,
  folderRole,
  formatDate,
  formatFullDate,
  formatRecipients,
  formatSize,
  inboxOwnerLabel,
  providerLabel,
  splitAddresses,
} from './format'
export { groupEmailsByThread, sortThreadGroups, getThreadParticipants, mergeThreadEmails } from './thread-utils'
export { EMAIL_SANITIZE_CONFIG, sanitizeEmailHtml, hasRemoteSrc, mailFrameDocument, escapeHtml } from './sanitize'
export { MailReader, describeWriteResult, type MailReaderProps, type ConfirmOptions } from './views/MailReader'
export { InboxList, groupInboxes, type InboxGroups, type InboxOwnerGroup } from './views/InboxList'
export { FolderList } from './views/FolderList'
export { EmailList } from './views/EmailList'
export { ThreadListItem } from './views/ThreadListItem'
export { ThreadConversationView } from './views/ThreadConversationView'
export { EmailViewer, MessageBody, MessageActionBar, type MessageActions, type RenderHtml } from './views/EmailViewer'
export { Composer, type ComposeMode, type ComposeValues } from './views/Composer'
