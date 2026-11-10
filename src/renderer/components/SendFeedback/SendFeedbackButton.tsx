import { lazy, Suspense } from 'react'
import { FeedbackIcon, TOP_BAR_ICON_ACTIVE_CLASS, TOP_BAR_ICON_REST_CLASS } from '@/components/icons/K2Icons'
import { useSendFeedbackStore } from '@/stores/send-feedback'
import { SEND_FEEDBACK_LABEL } from '@/lib/product-feedback'

// The dialog loads on first open, not with the top bar.
const SendFeedbackDialog = lazy(() => import('./SendFeedbackDialog'))

const noDrag = {
  WebkitAppRegion: 'no-drag',
} as React.CSSProperties

/** Top-bar Send feedback to K2 (prd-k2-product-feedback-v1 PF3): right of
 *  Help in the shared right cluster, Help's size and classes, Lucide
 *  `message-square` squared. Opens the dialog on the window's server. The
 *  menus (PF4) open the same dialog, so Zen has it too: it is mounted once
 *  per window by `SendFeedbackHost` (App.tsx), never by a bar. */
export default function SendFeedbackButton(): React.JSX.Element {
  const open = useSendFeedbackStore((s) => s.open)
  const openDialog = useSendFeedbackStore((s) => s.openDialog)
  return (
    <button
      type="button"
      data-testid="send-feedback-button"
      onClick={() => openDialog()}
      aria-pressed={open}
      aria-haspopup="dialog"
      className={`flex h-6 w-6 items-center justify-center ${open ? TOP_BAR_ICON_ACTIVE_CLASS : TOP_BAR_ICON_REST_CLASS} transition-colors no-drag`}
      style={noDrag}
      title={SEND_FEEDBACK_LABEL}
      aria-label={SEND_FEEDBACK_LABEL}
    >
      <FeedbackIcon />
    </button>
  )
}

/** The dialog, once per window, wherever the store opens it from (the
 *  button, the macOS app menu, the in-window Menu). */
export function SendFeedbackHost(): React.JSX.Element | null {
  const open = useSendFeedbackStore((s) => s.open)
  if (!open) return null
  return (
    <Suspense fallback={null}>
      <SendFeedbackDialog />
    </Suspense>
  )
}
