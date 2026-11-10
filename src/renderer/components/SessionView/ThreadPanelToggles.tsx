import type { JSX } from 'react'
import { floatProps } from './threadFloats'
import { THREAD_PANEL_KINDS, THREAD_PANEL_LABEL, type ThreadPanelKind } from './threadPanels'

const TOOLTIP: Record<ThreadPanelKind, string> = {
  chatter: 'messages between agents',
  heartbeats: 'recent heartbeats and other outside input',
}

/** Thread view's Chatter and Heartbeats buttons, after the terminal's. Each
 *  slides its panel out where the terminal would be; opening one closes the
 *  other and the terminal (`useSessionViewTab`). */
export function ThreadPanelToggles({
  open,
  onChange,
  kinds = THREAD_PANEL_KINDS,
}: {
  open: ThreadPanelKind | null
  onChange: (next: ThreadPanelKind | null) => void
  /** The buttons to draw (the header's place decides: none in split view). */
  kinds?: readonly ThreadPanelKind[]
}): JSX.Element {
  return (
    <>
      {kinds.map((kind) => {
        const on = open === kind
        const label = THREAD_PANEL_LABEL[kind]
        return (
          <button
            key={kind}
            type="button"
            data-testid={`thread-panel-toggle-${kind}`}
            data-thread-panel-toggle={kind}
            {...floatProps(true, 'round')}
            aria-pressed={on}
            aria-label={on ? `Hide ${label}` : `Show ${label}`}
            title={on ? `Hide ${label}` : `${label}: ${TOOLTIP[kind]}`}
            onClick={() => onChange(on ? null : kind)}
            className="no-drag cursor-pointer inline-flex items-center justify-center flex-shrink-0"
          >
            {kind === 'chatter' ? <ChatterIcon /> : <HeartbeatIcon />}
          </button>
        )
      })}
    </>
  )
}

function ChatterIcon(): JSX.Element {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3 5h11v8H8l-3 3v-3H3z" />
      <path d="M17 9h4v8h-2v3l-3-3h-5v-1" />
    </svg>
  )
}

function HeartbeatIcon(): JSX.Element {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M2 12h5l2-5 4 10 2-5h7" />
    </svg>
  )
}
