// 0.40.22 large-file transfers — the in-flight transfer overlay.
//
// Renders every active entry in the transfer-progress store as a compact
// card: kind glyph + filename + determinate percent bar (the
// LocalLLMSettings download-bar pattern) + Cancel. Anchored bottom-LEFT so
// it never fights the Toast stack (bottom-right); mounted alongside
// <Toast /> at each app root.

import { useTransferProgressStore } from '@/stores/transfer-progress'
import type { Transfer } from '@/stores/transfer-progress'

const KIND_LABEL: Record<Transfer['kind'], string> = {
  upload: 'Uploading',
  download: 'Downloading',
  compress: 'Compressing',
}

function TransferItem({ transfer }: { transfer: Transfer }): React.JSX.Element {
  const requestCancel = useTransferProgressStore((s) => s.requestCancel)
  const pct = transfer.fraction === null ? null : Math.round(transfer.fraction * 100)

  return (
    <div className="bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] text-xs shadow-lg min-w-[260px] max-w-[360px] overflow-hidden border border-[var(--color-border)]">
      <div className="px-3 py-2 flex items-center gap-2">
        <span className="flex-1 truncate leading-relaxed" title={transfer.label}>
          <span className="text-[var(--color-text-muted)]">
            {transfer.cancelRequested ? 'Cancelling' : KIND_LABEL[transfer.kind]}{' '}
          </span>
          {transfer.label}
        </span>
        <span className="flex-shrink-0 font-mono text-[10px] text-[var(--color-text-muted)]">
          {pct === null ? '…' : `${pct}%`}
        </span>
        {!transfer.cancelRequested && (
          <button
            className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] font-mono cursor-pointer transition-colors"
            onClick={() => requestCancel(transfer.id)}
          >
            Cancel
          </button>
        )}
      </div>
      <div className="h-[3px] bg-transparent">
        <div
          className="h-full transition-[width] duration-200"
          style={{
            width: pct === null ? '100%' : `${pct}%`,
            backgroundColor: 'var(--color-accent)',
            opacity: pct === null ? 0.25 : 0.8,
          }}
        />
      </div>
    </div>
  )
}

export default function TransferProgress(): React.JSX.Element | null {
  const transfers = useTransferProgressStore((s) => s.transfers)
  if (transfers.length === 0) return null

  return (
    <div className="fixed bottom-4 left-3 z-[9999] flex flex-col items-start gap-2 pointer-events-auto">
      {transfers.map((t) => (
        <TransferItem key={t.id} transfer={t} />
      ))}
    </div>
  )
}
