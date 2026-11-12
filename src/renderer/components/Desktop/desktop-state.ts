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
  /** A virtual desktop (prd-linux-virtual-desktop-v1 §6). */
  virtual?: VirtualDesktopInfo
}

/** What status says about a virtual desktop. */
export interface VirtualDesktopInfo {
  /** The agent holding it awake, and until when (Unix seconds). */
  hold?: { agent: string; until: number } | null
  /** How many desktops this server runs at once (the `waiting` card). */
  awakeLimit?: number
  /** No browser on the box: the Start card says it opens empty (LD-20). */
  noApps?: boolean
  /** No cgroup freezer: it closes instead of sleeping. */
  canSleep?: boolean
  /** Where K2 can't install the packages itself: the command to run by
   *  hand. */
  command?: string
}

/** One desktop this person may open (`GET /cli/desktop/list`). */
export interface DesktopListEntry {
  id: string
  /** This server's own desktop, or an agent's. */
  kind: 'server' | 'agent'
  /** The agent's name; "This server" for the server's. */
  name: string
  /** ready, asleep, closed… */
  state: string
  /** An agent's desktop: its workspace id and folder. */
  workspaceId?: string
  path?: string
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** `GET /cli/desktop/list`, read so an odd reply is a plain error and an
 *  entry K2 can't read is dropped. */
export function readDesktopList(body: unknown): DesktopListEntry[] {
  if (!isObj(body) || !Array.isArray(body.desktops)) throw new Error('the reply didn’t list any desktops')
  const out: DesktopListEntry[] = []
  for (const d of body.desktops) {
    if (!isObj(d) || typeof d.id !== 'string' || !d.id) continue
    const kind = d.kind === 'agent' ? 'agent' : 'server'
    const name = typeof d.name === 'string' && d.name ? d.name : kind === 'server' ? 'This server' : d.id
    const entry: DesktopListEntry = { id: d.id, kind, name, state: typeof d.state === 'string' ? d.state : '' }
    if (typeof d.workspaceId === 'string' && d.workspaceId) entry.workspaceId = d.workspaceId
    if (typeof d.path === 'string' && d.path) entry.path = d.path
    out.push(entry)
  }
  return out
}

/** The picker's word for a desktop's state (§6: awake, asleep, not started). */
export function desktopStateLabel(state: string): string {
  switch (state) {
    case 'ready':
    case 'starting':
    case 'waking':
      return 'awake'
    case 'asleep':
      return 'asleep'
    case 'waiting':
      return 'waiting'
    default:
      return 'not started'
  }
}

/** The first desktop a new tab shows (§6): in an agent's workspace whose
 *  switch is on (its desktop is listed), that agent's; otherwise This
 *  server. */
export function firstDesktop(list: readonly DesktopListEntry[], workspace: { id?: string | null; path?: string | null }): DesktopListEntry | null {
  const { id, path } = workspace
  const mine = list.find(
    (d) => d.kind === 'agent' && ((!!id && (d.workspaceId === id || d.id === `virtual:${id}`)) || (!!path && d.path === path)),
  )
  if (mine) return mine
  return list.find((d) => d.kind === 'server') ?? list[0] ?? null
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
  | VirtualState
  | 'ready'

/** A virtual desktop's states (prd-linux-virtual-desktop-v1 §3.4, §6). */
export type VirtualState =
  | 'not_set_up'
  | 'setting_up'
  | 'closed'
  | 'starting'
  | 'asleep'
  | 'waking'
  | 'waiting'
  | 'gone'
  | 'failed'
  | 'not_possible'

export const VIRTUAL_STATES: ReadonlySet<string> = new Set<VirtualState>([
  'not_set_up',
  'setting_up',
  'closed',
  'starting',
  'asleep',
  'waking',
  'waiting',
  'gone',
  'failed',
  'not_possible',
])

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
export function tabState(status: DesktopStatus, virtualDesktops = false): TabState {
  if (!status.enabled) return 'off'
  if (status.you.sameMachine) return 'same_machine'
  if (!status.you.mayView) return 'not_allowed'
  if (MACHINE_STATES.has(status.state)) return status.state as MachineState
  // The virtual desktop states, only from a server that lists the feature
  // (LD-16): without it no Start or Set up shows.
  if (virtualDesktops && VIRTUAL_STATES.has(status.state)) return status.state as VirtualState
  return 'unavailable'
}

/** The boot-status feature key for virtual desktops (LD-16). */
export const VIRTUAL_DESKTOP_FEATURE = 'virtual-desktop-v1' as const

/** A card's one action. */
export type CardAction = 'turn-on' | 'ask-at-mac' | 'cancel' | 'try-again' | 'retry' | 'set-up' | 'start'

export interface CardSpec {
  message: string
  /** What a Member (watch only) reads instead, where it differs. */
  memberMessage?: string
  /** The action, when this viewer may take it: `ownerOnly` for an Owner,
   *  `controllers` for an Owner or Admin. */
  action?: { id: CardAction; label: string; ownerOnly: boolean; controllers?: boolean }
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
  // A virtual desktop (prd-linux-virtual-desktop-v1 §6).
  not_set_up: {
    message: 'This server has no screen. K2 can start a desktop on it once three small packages are installed.',
    action: { id: 'set-up', label: 'Set up', ownerOnly: true },
  },
  setting_up: { message: 'Installing the desktop packages…' },
  closed: {
    message: 'This server has no screen. K2 can start a desktop on it.',
    memberMessage: "The desktop isn't running. An Owner or Admin can start it.",
    action: { id: 'start', label: 'Start desktop', ownerOnly: false, controllers: true },
  },
  starting: { message: 'Starting the desktop…' },
  asleep: {
    message: 'The desktop is asleep. Everything on it is kept.',
    memberMessage: 'The desktop is asleep. An Owner or Admin can wake it.',
  },
  waking: { message: 'Waking the desktop…' },
  waiting: { message: 'Waiting for another desktop to go to sleep.' },
  gone: { message: 'This desktop no longer exists.' },
  failed: {
    message: "K2 couldn't start the desktop.",
    action: { id: 'start', label: 'Retry', ownerOnly: false, controllers: true },
  },
  not_possible: { message: "K2 can't start a desktop on this computer." },
}

/** The card for `state` as THIS viewer sees it: an owner-only action is
 *  dropped for anyone who can't manage the desktop, an Owner-or-Admin one
 *  for anyone who can't control it (a Member), and a Member reads the
 *  Member sentence where the table has one. `mayControl` defaults to
 *  `mayManage`. */
export function cardFor(state: Exclude<TabState, 'ready'>, mayManage: boolean, mayControl: boolean = mayManage): CardSpec {
  const spec = STATE_CARDS[state]
  const action = spec.action
  const allowed = !action || (action.ownerOnly ? mayManage : action.controllers ? mayControl : true)
  const message = !mayControl && spec.memberMessage ? spec.memberMessage : spec.message
  if (allowed && message === spec.message) return spec
  return allowed && action ? { message, action } : { message }
}

export const NO_APPS_LINE = 'Opens with no apps. Agents can put a browser on it.'
export const NO_SLEEP_LINE = "This server can't pause a desktop, so it closes when nobody is viewing it."

/** A card's words with the daemon's own filled in (§6): a closed desktop's
 *  reason, a failure's reason, why it can't run here, how many run at once,
 *  and (LD-6) the daemon's sentence on the `unavailable` card. `notes` are
 *  the smaller lines under it. */
export function cardText(
  state: Exclude<TabState, 'ready'>,
  status: Pick<DesktopStatus, 'message' | 'virtual'> | null,
  mayManage: boolean,
  mayControl: boolean = mayManage,
): { line: string; notes: string[] } {
  const base = cardFor(state, mayManage, mayControl).message
  const msg = status?.message?.trim() ?? ''
  const v = status?.virtual
  const notes: string[] = []
  switch (state) {
    case 'closed':
      if (mayControl && v?.noApps) notes.push(NO_APPS_LINE)
      if (mayControl && v?.canSleep === false) notes.push(NO_SLEEP_LINE)
      if (!msg) return { line: base, notes }
      // Closed with a reason: the reason is the line; a Member is also told
      // who can start it.
      return mayControl ? { line: msg, notes } : { line: msg, notes: [base] }
    case 'waiting':
      return { line: v?.awakeLimit ? `${base} This server runs ${v.awakeLimit} at a time.` : base, notes }
    case 'failed':
    case 'unavailable':
      return { line: msg || base, notes }
    case 'not_set_up':
      if (v?.command) notes.push(`To install them by hand: ${v.command}`)
      return { line: base, notes }
    case 'not_possible':
      if (v?.command) notes.push(v.command)
      return { line: msg ? `K2 can't start a desktop on this computer: ${msg.replace(/\.$/, '')}.` : base, notes }
    default:
      return { line: base, notes }
  }
}

/** The S14 sentence the off card shows before turning it on. */
export const DESKTOP_ENABLE_CARD =
  "Anyone you allow can see and do anything you can on this computer, including your mail, files and passwords. Owners and Admins can open it; Members only with your grant, and only to watch. It stays on until an Owner turns it off. On Linux, K2 can't hide password apps from the picture, and notifications (2FA codes too) show in it."

/** The `virtual` switch's card (§4 item 2, LD-18), word for word as the
 *  daemon's constant. */
export const VIRTUAL_ENABLE_CARD =
  "K2 will start a desktop on this server. Anyone you allow can see and use whatever is opened on it. Programs on it run as this server's K2 user. It stays on until an Owner turns it off."

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
