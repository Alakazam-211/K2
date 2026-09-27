import { useState, type JSX, type MouseEvent } from 'react'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'

const PANE_LABEL: Record<SplitPaneView, string> = {
  chat: 'Chat',
  terminal: 'Terminal',
  thread: 'Thread',
  chatter: 'Chatter',
}

/** Button text for the active mode. Same words as the menu rows. */
const VIEW_LABEL: Record<SessionViewTab, string> = {
  terminal: PANE_LABEL.terminal,
  chat: PANE_LABEL.chat,
  thread: PANE_LABEL.thread,
  chatter: PANE_LABEL.chatter,
  split: 'Split view',
}

function viewMenuItems(chatEligible: boolean): ContextMenuItemDef[] {
  return [
    { id: 'terminal', label: VIEW_LABEL.terminal },
    { id: 'chat', label: VIEW_LABEL.chat, badge: 'Beta', enabled: chatEligible },
    { id: 'thread', label: VIEW_LABEL.thread },
    { id: 'chatter', label: VIEW_LABEL.chatter },
    { id: 'split', label: VIEW_LABEL.split },
  ]
}

function sideMenuItems(chatEligible: boolean): ContextMenuItemDef[] {
  return [
    { id: 'terminal', label: 'Terminal' },
    { id: 'chat', label: 'Chat', enabled: chatEligible },
    { id: 'thread', label: 'Thread' },
    { id: 'chatter', label: 'Chatter' },
  ]
}

function isSessionViewTab(id: string): id is SessionViewTab {
  return id === 'chat' || id === 'terminal' || id === 'thread' || id === 'chatter' || id === 'split'
}

function isSplitPaneView(id: string): id is SplitPaneView {
  return id === 'chat' || id === 'terminal' || id === 'thread' || id === 'chatter'
}

const BUTTON_CLASS =
  'self-center inline-flex items-center gap-1 px-2 py-0.5 text-[11px] font-medium rounded flex-shrink-0 cursor-pointer text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]'

/**
 * One view button. Its label is the active mode, not the word View.
 * The menu is anchored under the button.
 * Esc and a click outside leave the current view alone.
 * Split choosers sit to the right and are hidden otherwise.
 */
export function SessionViewMenu({
  value,
  splitLeft,
  splitRight,
  chatEligible,
  onChange,
  onSplitLeft,
  onSplitRight,
}: {
  value: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  chatEligible: boolean
  onChange: (tab: SessionViewTab) => void
  onSplitLeft: (view: SplitPaneView) => void
  onSplitRight: (view: SplitPaneView) => void
}): JSX.Element {
  const [openKind, setOpenKind] = useState<'view' | 'left' | 'right' | null>(null)

  async function openAt(
    event: MouseEvent<HTMLButtonElement>,
    kind: 'view' | 'left' | 'right',
    items: ContextMenuItemDef[],
  ): Promise<string | null> {
    const rect = event.currentTarget.getBoundingClientRect()
    setOpenKind(kind)
    try {
      return await useContextMenuStore.getState().show(rect.left, rect.bottom, items)
    } finally {
      setOpenKind((current) => (current === kind ? null : current))
    }
  }

  return (
    <div
      className="flex items-center gap-1 self-stretch flex-shrink-0"
      data-testid="session-view-menu"
      data-view={value}
      data-split-left={splitLeft}
      data-split-right={splitRight}
    >
      <button
        type="button"
        data-testid="session-view-button"
        aria-haspopup="menu"
        aria-expanded={openKind === 'view'}
        className={BUTTON_CLASS}
        onClick={(event) => {
          void openAt(event, 'view', viewMenuItems(chatEligible)).then((picked) => {
            if (!picked || !isSessionViewTab(picked)) return
            if (picked === 'chat' && !chatEligible) return
            onChange(picked)
          })
        }}
      >
        {VIEW_LABEL[value]}
        <svg
          width="10"
          height="10"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
          className="flex-shrink-0"
        >
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      {value === 'split' ? (
        <>
          <SideChooser
            testId="session-view-split-left"
            label={PANE_LABEL[splitLeft]}
            ariaLabel={`Left side ${PANE_LABEL[splitLeft]}`}
            expanded={openKind === 'left'}
            onClick={(event) => {
              void openAt(event, 'left', sideMenuItems(chatEligible)).then((picked) => {
                if (!picked || !isSplitPaneView(picked)) return
                if (picked === 'chat' && !chatEligible) return
                onSplitLeft(picked)
              })
            }}
          />
          <SideChooser
            testId="session-view-split-right"
            label={PANE_LABEL[splitRight]}
            ariaLabel={`Right side ${PANE_LABEL[splitRight]}`}
            expanded={openKind === 'right'}
            onClick={(event) => {
              void openAt(event, 'right', sideMenuItems(chatEligible)).then((picked) => {
                if (!picked || !isSplitPaneView(picked)) return
                if (picked === 'chat' && !chatEligible) return
                onSplitRight(picked)
              })
            }}
          />
        </>
      ) : null}
    </div>
  )
}

function SideChooser({
  testId,
  label,
  ariaLabel,
  expanded,
  onClick,
}: {
  testId: string
  label: string
  ariaLabel: string
  expanded: boolean
  onClick: (event: MouseEvent<HTMLButtonElement>) => void
}): JSX.Element {
  return (
    <button
      type="button"
      data-testid={testId}
      aria-haspopup="menu"
      aria-expanded={expanded}
      aria-label={ariaLabel}
      className={`${BUTTON_CLASS} text-[var(--color-text-secondary)]`}
      onClick={onClick}
    >
      {label}
      <svg
        width="10"
        height="10"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className="flex-shrink-0"
      >
        <path d="M6 9l6 6 6-6" />
      </svg>
    </button>
  )
}
