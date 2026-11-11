// The Mail page's body (lazy chunk): @k2/mail-reader wired to this window's
// server, K2's inert frame, K2's confirm dialog and download, and K2's own
// controls and icons (mail-ui).

import { useMemo } from 'react'
import { MailReader, type ReaderAttachmentFile } from '@k2/mail-reader'
import { primaryScope } from '@/kessel/server-scope'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { saveBlobInBrowser } from '@/lib/fs-transfer'
import { daemonMailReaderSource } from './mail-reader-source'
import { renderInertMail } from './InertMailFrame'
import { K2_MAIL_UI } from './mail-ui'

/** The thin bar over the reader (prd-ocmail-admin-in-k2-v1 §5.2). */
export const MAIL_READER_BAR = 'Opening a message here doesn’t mark it read. Every access is logged.'

export default function MailReaderHost({ hostKey }: { hostKey: string }): React.JSX.Element {
  // A new source per server: a host switch starts the reader over.
  const source = useMemo(() => daemonMailReaderSource(primaryScope()), [hostKey])
  return (
    <MailReader
      key={hostKey}
      source={source}
      ui={K2_MAIL_UI}
      renderHtml={renderInertMail}
      confirm={(o) =>
        useConfirmDialogStore.getState().confirm({
          title: o.title,
          message: o.message,
          confirmLabel: o.confirmLabel,
          destructive: o.destructive,
        })
      }
      saveAttachment={(f: ReaderAttachmentFile) => saveBlobInBrowser(f.blob, f.filename)}
      banner={
        <div
          data-testid="mail-reader-bar"
          className="px-4 py-1 type-hint text-[var(--color-text-muted)] border-b border-[var(--color-border)] bg-[var(--color-bg-inset)]"
        >
          {MAIL_READER_BAR}
        </div>
      }
    />
  )
}
