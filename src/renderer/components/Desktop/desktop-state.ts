// The Desktop tab's state (prd-remote-desktop-tab-v1 §8): the daemon's
// `GET /cli/desktop/status` shape, the pure `tabState`, and the states table
// (what each card says and its one action). The daemon owns every rule;
// this only decides which card to draw.
//
// Pure: no React, no DOM.

import type { Lane } from './desktop-wire'

/** The machine's state as `status.state` reports it (even while off). */
export type MachineState =
  | 'ready'
  | 'needs_permission'
  | 'no_session'
  | 'no_display'
  | 'locked'
  | 'portal_allow'
  | 'needs_update'
  | 'blocked_in_login_items'
  | 'unavailable'

export type DesktopRole = 'owner' | 'admin' | 'member' | 'owner-token' | 'shell'

export interface DesktopGrant {
  user: string
  userId: string
  mode: 'view' | 'control'
  grantedBy: string
  /** Unix seconds. */
  grantedAt: number
}

export interface DesktopViewer {
  id: string
  user: string
  role: string
  mode: 'view' | 'control'
  ingress: string
  ip: string
  device: string
  display: string
  /** Unix ms. */
  since: number
}

export interface DesktopStatus {
  desktop: string
  machine: string
  enabled: boolean
  backend: 'macos' | 'linux' | 'windows' | string
  state: MachineState | string
  message: string
  lane: Lane
  you: {
    role: DesktopRole | string
    mayView: boolean
    mayControl: boolean
    mayManage: boolean
    sameMachine: boolean
  }
  /** Only when `you.mayManage`. */
  hideApps?: string[]
  grants?: DesktopGrant[]
  viewers?: DesktopViewer[]
}

/** Every card the tab can show. `running` is the picture (no card). */
export type TabState =
  | 'off'
  | 'not_allowed'
  | 'needs_permission'
  | 'waiting_consent'
  | 'denied'
  | 'locked'
  | 'no_session'
  | 'no_display'
  | 'portal_allow'
  | 'same_machine'
  | 'needs_update'
  | 'unavailable'
  | 'blocked_in_login_items'
  | 'update_k2'
  | 'ready'

const MACHINE_STATES: ReadonlySet<string> = new Set<MachineState>([
  'ready',
  'needs_permission',
  'no_session',
  'no_display',
  'locked',
  'portal_allow',
  'needs_update',
  'blocked_in_login_items',
  'unavailable',
])

/** Which card a status means for THIS viewer. Order matters: off first
 *  (nothing to see), then this machine (pointless to watch yourself), then
 *  access, then the machine's own state. A state this client doesn't know
 *  (a newer daemon) reads as `unavailable`, never as `ready`. */
export function tabState(status: DesktopStatus): TabState {
  if (!status.enabled) return 'off'
  if (status.you.sameMachine) return 'same_machine'
  if (!status.you.mayView) return 'not_allowed'
  return MACHINE_STATES.has(status.state) ? (status.state as MachineState) : 'unavailable'
}

/** A card's one action. */
export type CardAction = 'turn-on' | 'ask-at-mac' | 'cancel' | 'try-again' | 'retry'

export interface CardSpec {
  message: string
  /** The action, when this viewer may take it. */
  action?: { id: CardAction; label: string; ownerOnly: boolean }
}

/** The states table (§8, plus `unavailable`, `blocked_in_login_items` and
 *  the older-daemon card). `ready` is not a card: the pane draws the
 *  toolbar and the picture there. */
export const STATE_CARDS: Readonly<Record<Exclude<TabState, 'ready'>, CardSpec>> = {
  off: {
    message: 'Remote desktop is off on this computer.',
    action: { id: 'turn-on', label: 'Turn on', ownerOnly: true },
  },
  not_allowed: { message: "You don't have access to this computer's screen." },
  needs_permission: {
    message: 'Someone at this Mac has to allow K2 Screen once.',
    action: { id: 'ask-at-mac', label: 'Ask at the Mac', ownerOnly: true },
  },
  waiting_consent: {
    message: 'Waiting for someone at the screen to allow it.',
    action: { id: 'cancel', label: 'Cancel', ownerOnly: false },
  },
  denied: {
    message: 'The person at the screen said no.',
    action: { id: 'try-again', label: 'Try again', ownerOnly: false },
  },
  locked: { message: "This Mac is locked. K2 can't unlock it." },
  no_session: { message: "No one is logged in on this computer's screen." },
  no_display: { message: 'This computer has no screen K2 can show.' },
  portal_allow: { message: 'Someone at this computer has to click Allow once.' },
  same_machine: { message: "You're on this computer." },
  needs_update: {
    message: "K2 Screen and K2 don't match; restarting it.",
    action: { id: 'retry', label: 'Retry', ownerOnly: false },
  },
  unavailable: { message: "K2 Screen isn't in this build of K2 yet." },
  blocked_in_login_items: { message: 'Turn K2 Screen on in System Settings → General → Login Items.' },
  update_k2: { message: 'Update K2 on this computer to use Remote desktop.' },
}

/** The card for `state` as THIS viewer sees it: an owner-only action is
 *  dropped for anyone who can't manage the desktop. */
export function cardFor(state: Exclude<TabState, 'ready'>, mayManage: boolean): CardSpec {
  const spec = STATE_CARDS[state]
  if (spec.action?.ownerOnly && !mayManage) return { message: spec.message }
  return spec
}

/** The S14 sentence the off card shows before turning it on. */
export const DESKTOP_ENABLE_CARD =
  "Anyone you allow can see and do anything you can on this computer, including your mail, files and passwords. Owners and Admins can open it; Members only with your grant, and only to watch. It stays on until an Owner turns it off. On Linux, K2 can't hide password apps from the picture, and notifications (2FA codes too) show in it."

/** The boot-status feature key that says the routes exist. */
export const DESKTOP_FEATURE = 'remote-desktop-v1' as const

/** Does `open_desktop`'s `for` name this client? `owner` is this computer's
 *  own app (the owner token), and any Owner login in `owners` (the owner
 *  asked from a K2 shell tab they watch from another device); a remote room
 *  matches its login's username. */
export function openDesktopIsForMe(
  forField: unknown,
  isRemote: boolean,
  username: string | undefined,
  owners?: unknown,
): boolean {
  if (typeof forField !== 'string' || !forField) return false
  if (!isRemote) return forField === 'owner'
  if (!username) return false
  if (forField === username) return true
  return forField === 'owner' && Array.isArray(owners) && owners.includes(username)
}
