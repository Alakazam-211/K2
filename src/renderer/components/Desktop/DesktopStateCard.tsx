// One Desktop tab card (prd-remote-desktop-tab-v1 §8): the state's line and
// at most its one action. The off card's Turn on shows the S14 sentence
// first and asks again before it sends.

import { useState } from 'react'
import { cardFor, DESKTOP_ENABLE_CARD, type CardAction, type TabState } from './desktop-state'

export interface DesktopStateCardProps {
  state: Exclude<TabState, 'ready'>
  /** This viewer manages the desktop (an Owner): owner-only actions show. */
  mayManage: boolean
  /** The machine's own words (`status.message`), shown under the line. */
  detail?: string
  /** The enable reply's card text, when the daemon sent one. */
  enableCard?: string
  busy?: boolean
  onAction(action: CardAction): void
}

export function DesktopStateCard(props: DesktopStateCardProps): React.JSX.Element {
  const spec = cardFor(props.state, props.mayManage)
  const [confirming, setConfirming] = useState(false)
  const action = spec.action
  const click = (): void => {
    if (!action) return
    if (action.id === 'turn-on' && !confirming) {
      setConfirming(true)
      return
    }
    setConfirming(false)
    props.onAction(action.id)
  }
  return (
    <div
      className="flex h-full w-full items-center justify-center p-6"
      data-desktop-card={props.state}
    >
      <div className="flex max-w-md flex-col items-center gap-3 text-center">
        <p className="text-sm text-[var(--color-text-primary)]" data-desktop-card-line="">
          {spec.message}
        </p>
        {props.detail && props.detail !== spec.message ? (
          <p className="text-xs text-[var(--color-text-muted)]" data-desktop-card-detail="">
            {props.detail}
          </p>
        ) : null}
        {confirming ? (
          <p className="border border-[var(--color-border)] p-3 text-left text-xs text-[var(--color-text-secondary)]" data-desktop-enable-card="">
            {props.enableCard || DESKTOP_ENABLE_CARD}
          </p>
        ) : null}
        {action ? (
          <button
            type="button"
            className="h-7 cursor-pointer border border-[var(--color-border)] px-3 text-xs text-[var(--color-text-primary)] transition-colors enabled:hover:border-[var(--color-text-muted)] disabled:cursor-default disabled:opacity-50"
            data-desktop-card-action={action.id}
            disabled={props.busy}
            onClick={click}
          >
            {action.id === 'turn-on' && confirming ? 'Turn on Remote desktop' : action.label}
          </button>
        ) : null}
      </div>
    </div>
  )
}
