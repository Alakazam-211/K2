# Where this package comes from

- **Source repo:** `a private fork` (OCmail, the Office Craft agent's product), a fork of
  `root-fr/jmap-webmail` v1.7.3 (MIT).
- **Branch:** `tauri`
- **Commit:** `private-fork-commit` (the commit where Office Craft declared its
  own changes MIT; LICENSE carries both copyright lines).
- **Not used:** `ocmail` `main` (pure upstream), and no AGPL webmail (Bulwark or any other) was read.
- **Licence:** MIT; both copyright lines and the permission text are in `NOTICE`.
- **Updates:** pulled by hand, file by file, the way OCmail pulls jmap-webmail. Ask Office Craft before
  splitting pieces out of OCmail itself.

## Files adapted

| OCmail file (at the private fork commit) | Here | What was kept |
|---|---|---|
| `lib/jmap/types.ts` | `src/types.ts` | `Email`, `EmailAddress`, `Attachment`, `Mailbox`, `ThreadGroup`, `AuthenticationResults` shapes, trimmed to what K2's daemon serves (no JMAP account ids, blob ids, keywords maps, rights, quotas, contacts or calendars). |
| `lib/thread-utils.ts` | `src/thread-utils.ts` | `groupEmailsByThread`, `sortThreadGroups`, `getThreadParticipants`, `mergeThreadEmails` (account scoping dropped: K2 message ids are already unique per inbox). |
| `lib/email-sanitization.ts` | `src/sanitize.ts` | The DOMPurify config (forbidden tags and handlers) and `escapeHtml`; K2 adds: no remote `src`/`srcset`/`background`, no `url(http…)` in styles, links lose `href` and keep their target in `title`. |
| `components/layout/sidebar.tsx` (folder list part only) | `src/views/FolderList.tsx`, `src/format.ts` (`folderOrder`) | Role-first folder order and role names. |
| `components/email/email-list.tsx` | `src/views/EmailList.tsx` | Thread-grouped list, counts header, empty/loading states, "no more" end. |
| `components/email/email-list-item.tsx`, `thread-list-item.tsx`, `thread-email-item.tsx` | `src/views/ThreadListItem.tsx` | Unread dot, sender/participants line, count, subject, date, attachment mark. |
| `components/email/thread-conversation-view.tsx` | `src/views/ThreadConversationView.tsx` | Oldest-first cards, the newest expanded, collapsed cards show sender and date. |
| `components/email/email-viewer.tsx` (read parts) | `src/views/EmailViewer.tsx`, `src/format.ts` (`formatRecipients`, `formatSize`) | Header block, recipients line, SPF/DKIM/DMARC badges, attachment list. |

## What was removed or changed

- The JMAP client and every store (`email-store`, `auth-store`, `settings-store`, `theme-store`):
  replaced by one interface, `MailReaderSource` (`src/source.ts`). The views make no network calls.
- `next-intl` (`useTranslations`): replaced by an English `t()` table (`src/i18n.ts`) so other
  languages can come back later.
- `next/*`, `lucide-react`, `@tanstack/react-virtual`, `date-fns`, shadcn `cn()`/UI kit: not used. The list
  pages with offset + "Load more"; dates use `Intl`; icons are left to the host (K2 Design).
- OCmail's mail iframe (`sandboxed-email-frame.tsx`, `sandbox="allow-same-origin"`): not copied. The
  host renders HTML through its own frame (`renderHtml`); K2 passes `HtmlFrame profile="inert"`
  (no script, no network, no same-origin).
- The "load remote images" path and trusted-sender lists: gone. Remote content is never loaded.
- Marking read on open (`markAsRead` in `app/[locale]/page.tsx`, the thread-expand mark): gone. Opening
  never changes read state; only the explicit Mark read / Mark unread button calls `source.flag`.
- Context menus, drag and drop, multi-select batch actions, colour tags, folder create/rename/delete,
  spam actions, Sieve, vacation, contacts, calendar, identities, signatures: gone.
- Write actions are explicit buttons only (Reply, New message, Send, Save draft, Move, Archive,
  Delete to Trash, Star, Mark read/unread), and Send and Delete ask first.
- Colours: K2 CSS variables only (`--color-*`); OCmail's `.dark` class and tokens are not used.
