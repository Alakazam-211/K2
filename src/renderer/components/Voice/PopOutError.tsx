// Pop out failed: the reason, on the stage where Pop out was pressed, with
// Try again and Dismiss. Never silent; the call stays in this window.

import { Button } from '@/components/ui/Button'
import { dismissCallPopoutError, popOutCall, useCallPopout } from '@/stores/call-popout'

export default function PopOutError(): React.JSX.Element | null {
  const error = useCallPopout((s) => s.error)
  if (!error) return null
  return (
    <div role="alert" data-testid="call-popout-error" className="flex items-center gap-2 border-b border-[var(--color-border)] px-2 py-1 text-[11px] text-[var(--color-status-error-text)]">
      <span className="min-w-0 flex-1">{error}</span>
      <Button variant="quiet" data-testid="call-popout-retry" onClick={() => void popOutCall()}>
        Try again
      </Button>
      <Button variant="quiet" data-testid="call-popout-dismiss" onClick={dismissCallPopoutError}>
        Dismiss
      </Button>
    </div>
  )
}
