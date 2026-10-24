import { useState, type JSX, type MouseEvent } from 'react'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import type { SessionViewTab, SplitPaneView } from './sessionViewTab'
import type { ServerScope } from '@/kessel/server-scope'
import { unreadSupported, useUnreadEntry } from '@/stores/unread'
import { muteMenuState, toggleMute } from '@/components/common/UnreadBadge'

// The Chat view is retired from the UI in 0.46.4 (Rosson 2026-10-23: Threads
// instead). It has no row in either menu below, a picked `chat` id is
// ignored, and live view state never holds `chat` (see `retireChatTab`).
// The `chat` label stays only because the types still name `chat` until the
// Chat view code goes in 0.46.5 (then delete it here too).
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

const MUTE_THREAD_ID = 'mute-thread'

/** No unread count, unread dot or waiting count on the button or its
 *  rows: those live in the main nav (Rosson 2026-10-21). The menu keeps
 *  only the Mute / Unmute Thread item. No Chat row (retired 0.46.4). */
export function viewMenuItems(
  thread?: { muted: boolean; canMute: boolean },
): ContextMenuItemDef[] {
  const items: ContextMenuItemDef[] = [
    { id: 'terminal', label: VIEW_LABEL.terminal },
    { id: 'thread', label: VIEW_LABEL.thread },
    { id: 'chatter', label: VIEW_LABEL.chatter },
    { id: 'split', label: VIEW_LABEL.split },
  ]
  if (thread?.canMute) {
    items.push({ id: 'separator-mute', label: '', type: 'separator' })
    items.push({ id: MUTE_THREAD_ID, label: muteMenuState(thread.muted, 'Thread').label })
  }
  return items
}

/** A Split side's choices. No Chat row (retired 0.46.4). */
export function sideMenuItems(): ContextMenuItemDef[] {
  return [
    { id: 'terminal', label: 'Terminal' },
    { id: 'thread', label: 'Thread' },
    { id: 'chatter', label: 'Chatter' },
  ]
}

/** A menu pick the view button acts on. Not `chat` (retired 0.46.4). */
function isSessionViewTab(id: string): id is SessionViewTab {
  return id === 'terminal' || id === 'thread' || id === 'chatter' || id === 'split'
}

/** A Split side pick the choosers act on. Not `chat` (retired 0.46.4). */
function isSplitPaneView(id: string): id is SplitPaneView {
  return id === 'terminal' || id === 'thread' || id === 'chatter'
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
  onChange,
  onSplitLeft,
  onSplitRight,
  threadConversationId,
  threadScope,
}: {
  value: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  onChange: (tab: SessionViewTab) => void
  onSplitLeft: (view: SplitPaneView) => void
  onSplitRight: (view: SplitPaneView) => void
  /** This chat's Thread (thread-read-v1): the menu can mute it. Its
   *  unread count and waiting marker are not drawn here (main nav only). */
  threadConversationId?: string | null
  /** The room's server; the unread store reads only the window's. */
  threadScope?: ServerScope
}): JSX.Element {
  const [openKind, setOpenKind] = useState<'view' | 'left' | 'right' | null>(null)
  const threadEntry = useUnreadEntry('thread', threadConversationId, threadScope)
  const canMute =
    Boolean(threadConversationId) &&
    (threadScope === undefined || threadScope.isWindowHost()) &&
    unreadSupported()

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
          void openAt(event, 'view', viewMenuItems({ muted: threadEntry?.muted === true, canMute })).then((picked) => {
            if (picked === MUTE_THREAD_ID && threadConversationId) {
              const { next } = muteMenuState(threadEntry?.muted === true, 'Thread')
              void toggleMute('thread', threadConversationId, next, 'Thread')
              return
            }
            if (!picked || !isSessionViewTab(picked)) return
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
              void openAt(event, 'left', sideMenuItems()).then((picked) => {
                if (!picked || !isSplitPaneView(picked)) return
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
              void openAt(event, 'right', sideMenuItems()).then((picked) => {
                if (!picked || !isSplitPaneView(picked)) return
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
