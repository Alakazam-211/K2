// A tiny string table in place of OCmail's next-intl, English only for now.
// Keys keep OCmail's names where a string came from there, so another
// language can be added back as a second table later.

const EN = {
  // inboxes
  inboxes: 'Inboxes',
  your_mailboxes: 'Your mailboxes',
  all_inboxes: 'All inboxes',
  no_inboxes: 'No inboxes to show.',
  no_own_inboxes: 'You have no mailboxes on this server yet.',
  kind_agent: 'Agent',
  kind_person: 'Person',
  provider_hosted: 'Hosted',
  provider_gmail: 'Gmail',
  provider_microsoft: 'Microsoft',
  provider_imap: 'IMAP',
  owner_unknown: 'Unknown owner',
  // folders
  folders: 'Folders',
  folder_inbox: 'Inbox',
  folder_sent: 'Sent',
  folder_drafts: 'Drafts',
  folder_junk: 'Junk',
  folder_trash: 'Trash',
  folder_archive: 'Archive',
  // list
  loading: 'Loading…',
  loading_more: 'Loading more…',
  load_more: 'Load more',
  no_emails: 'No messages here',
  no_emails_description: 'Nothing in this folder matches.',
  no_more_emails: 'No more messages',
  conversations_count_simple: '{count} conversations',
  conversations_count_plus: '{count}+ conversations',
  search_placeholder: 'Search subject and body',
  unread_only: 'Unread only',
  refresh: 'Refresh',
  pick_inbox: 'Pick an inbox.',
  pick_message: 'Pick a message.',
  // viewer
  no_subject: '(no subject)',
  unknown_sender: 'Unknown',
  from: 'From',
  to: 'To',
  cc: 'Cc',
  recipient_and_others: '{name} and {count} others',
  attachments: 'Attachments',
  download: 'Download',
  downloading: 'Downloading…',
  remote_blocked: 'Remote images and content are off in this reader.',
  links_inert: 'Links don’t open from here; hover one to see where it goes.',
  untrusted: 'External email — untrusted content.',
  messages_in_thread: '{count} messages',
  // actions
  reply: 'Reply',
  reply_all: 'Reply all',
  new_message: 'New message',
  mark_read: 'Mark read',
  mark_unread: 'Mark unread',
  star: 'Star',
  unstar: 'Unstar',
  move_to: 'Move to…',
  archive: 'Archive',
  delete: 'Delete',
  // composer
  send: 'Send',
  save_draft: 'Save draft',
  cancel: 'Cancel',
  subject: 'Subject',
  body: 'Message',
  to_placeholder: 'name@example.com, other@example.com',
  sending: 'Sending…',
  saving: 'Saving…',
  sent_ok: 'Sent.',
  sent_pending: 'Waiting for approval before it goes out.',
  draft_saved: 'Draft saved.',
  confirm_send_title: 'Send this message?',
  confirm_send_message: 'It goes out from {inbox} to {to}.',
  confirm_delete_title: 'Delete this message?',
  confirm_delete_message: 'It moves to Trash in {inbox}.',
  done: 'Done.',
  error_prefix: 'Couldn’t do that: {message}',
} as const

export type MessageKey = keyof typeof EN

/** `t('conversations_count_simple', { count: 3 })` → "3 conversations". */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  let s: string = EN[key]
  if (params) {
    for (const [k, v] of Object.entries(params)) s = s.split(`{${k}}`).join(String(v))
  }
  return s
}
