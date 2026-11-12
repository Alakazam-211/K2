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
  // A virtual desktop (contract-virtual-desktop-wire-v1): absent on macOS
  // and for `console`.
  /** `server` (This server) or `agent`. */
  kind?: 'server' | 'agent' | string
  /** "This server", or the agent's name. */
  name?: string
  workspaceId?: string
  /** An agent's desktop: its workspace's "Allow a virtual desktop" switch. */
  agentsAllowed?: boolean
  /** Only with `closed`, when K2 closed it (`message` holds the sentence). */
  reason?: DesktopCloseReason | null
  virtual?: VirtualDesktopInfo
}

/** Who may start, wake and close a virtual desktop (`you.mayStart`):
 *  Owners and Admins, the owner token and a K2 terminal; never a Member.
 *  An older daemon doesn't say: there, nobody. */
export function mayStartOf(status: Pick<DesktopStatus, 'you'>): boolean {
  return (status.you as { mayStart?: unknown }).mayStart === true
}

export interface DesktopCloseReason {
  code: 'asleep_too_long' | 'low_memory' | 'restarted' | 'closed_by' | 'switched_off' | 'crashed' | 'nobody_viewing' | string
  /** Unix seconds. */
  at: number
  by?: string
  detail?: string
}

/** The `virtual` object of status. */
export interface VirtualDesktopInfo {
  size?: { width: number; height: number } | null
  display?: string | null
  memoryBytes?: number | null
  present?: Array<{ viewer: string; user: string }>
  /** The agent session holding it awake (its K2 address) and until when
   *  (Unix seconds); null when none. */
  hold?: { by: string; until: number } | null
  nextSleepAt?: number | null
  nextCloseAt?: number | null
  /** false: this server closes a desktop instead of pausing it. */
  canSleep?: boolean
  /** K2 Screen's own state on that display. */
  screen?: string
  /** Queued for an awake slot. */
  waiting?: { position: number; awakeLimit: number } | null
  /** "Separate, not private" (agent desktops only). */
  label?: string | null
  /** With `not_set_up`: the missing packages, and the command to run by
   *  hand (null when K2 can install them). */
  missing?: string[]
  manual?: string | null
  /** With `not_set_up`: what Set up installs. */
  setupCard?: string | null
  /** Awake 7 days: suggest closing and starting again. */
  suggestRecycle?: boolean
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
  /** An agent's desktop: its workspace id, and "Separate, not private". */
  workspaceId?: string
  label?: string
}

export interface DesktopList {
  /** The desktop a new tab in the asked workspace opens. */
  default: string
  desktops: DesktopListEntry[]
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** `GET /cli/desktop/list`, read so an odd reply is a plain error and an
 *  entry K2 can't read is dropped. */
export function readDesktopList(body: unknown): DesktopList {
  if (!isObj(body) || !Array.isArray(body.desktops)) throw new Error('the reply didn’t list any desktops')
  const desktops: DesktopListEntry[] = []
  for (const d of body.desktops) {
    if (!isObj(d) || typeof d.id !== 'string' || !d.id) continue
    const kind = d.kind === 'agent' ? 'agent' : 'server'
    const name = typeof d.name === 'string' && d.name ? d.name : kind === 'server' ? 'This server' : d.id
    const entry: DesktopListEntry = { id: d.id, kind, name, state: typeof d.state === 'string' ? d.state : '' }
    if (typeof d.workspaceId === 'string' && d.workspaceId) entry.workspaceId = d.workspaceId
    if (typeof d.label === 'string' && d.label) entry.label = d.label
    desktops.push(entry)
  }
  const dflt = typeof body.default === 'string' && body.default ? body.default : (desktops.find((d) => d.kind === 'server')?.id ?? '')
  return { default: dflt, desktops }
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

export const VIRTUAL_STATES: ReadonlySet<string> = new Set<VirtualState>([
  'not_set_up',
  'setting_up',
  'closed',
  'starting',
  'asleep',
  'waking',
  'waiting',
  'gone',
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
   *  `starters` for whoever may start a desktop (`you.mayStart`). */
  action?: { id: CardAction; label: string; ownerOnly: boolean; starters?: boolean }
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
    action: { id: 'start', label: 'Start desktop', ownerOnly: false, starters: true },
  },
  starting: { message: 'Starting the desktop…' },
  asleep: {
    message: 'The desktop is asleep. Everything on it is kept.',
    memberMessage: 'The desktop is asleep. An Owner or Admin can wake it.',
  },
  waking: { message: 'Waking the desktop…' },
  waiting: { message: 'Waiting for another desktop to go to sleep.' },
  gone: { message: 'This desktop no longer exists.' },
}

/** The card for `state` as THIS viewer sees it: an owner-only action is
 *  dropped for anyone who can't manage the desktop, a starter's one (Start)
 *  for anyone who may not start it (a Member), who also reads the Member
 *  sentence where the table has one. `mayStart` defaults to `mayManage`. */
export function cardFor(state: Exclude<TabState, 'ready'>, mayManage: boolean, mayStart: boolean = mayManage): CardSpec {
  const spec = STATE_CARDS[state]
  const action = spec.action
  const allowed = !action || (action.ownerOnly ? mayManage : action.starters ? mayStart : true)
  const message = !mayStart && spec.memberMessage ? spec.memberMessage : spec.message
  if (allowed && message === spec.message) return spec
  return allowed && action ? { message, action } : { message }
}

export const NO_SLEEP_LINE = "This server can't pause a desktop, so it closes when nobody is viewing it."
export const AGENT_SWITCH_OFF_LINE = 'This agent isn’t allowed a desktop yet. Turn on “Allow a virtual desktop” in its workspace settings.'

export interface CardText {
  line: string
  notes: string[]
  /** Replaces the table's action (`null`: none). */
  action?: CardSpec['action'] | null
}

/** A virtual desktop card's words with the daemon's filled in (contract
 *  `virtual-desktop-wire-v1`): a closed desktop's reason, the setup card
 *  and the command to run by hand, the place in the queue, and the
 *  daemon's `message` on `unavailable` (LD-6), which on a virtual desktop
 *  also gets Retry. `notes` are the smaller lines under it. */
export function cardText(
  state: Exclude<TabState, 'ready'>,
  status: Pick<DesktopStatus, 'message' | 'virtual' | 'reason' | 'kind' | 'agentsAllowed'> | null,
  mayManage: boolean,
  mayStart: boolean = mayManage,
  virtualDesktop = false,
): CardText {
  const base = cardFor(state, mayManage, mayStart).message
  const msg = status?.message?.trim() ?? ''
  const v = status?.virtual
  const notes: string[] = []
  switch (state) {
    case 'closed': {
      const agentOff = status?.kind === 'agent' && status.agentsAllowed === false
      if (agentOff) notes.push(AGENT_SWITCH_OFF_LINE)
      if (mayStart && v?.canSleep === false) notes.push(NO_SLEEP_LINE)
      const out: CardText = { line: base, notes }
      if (agentOff) out.action = null
      // Closed by K2 (`reason`): its sentence is the line; a Member is also
      // told who can start it.
      if (status?.reason && msg) {
        out.line = msg
        if (!mayStart) out.notes = [base, ...notes]
      }
      return out
    }
    case 'not_set_up':
      if (v?.setupCard) notes.push(v.setupCard)
      if (v?.manual) notes.push(`To install them by hand: ${v.manual}`)
      return { line: base, notes }
    case 'waiting': {
      const w = v?.waiting
      if (!w) return { line: base, notes }
      if (w.position > 0) notes.push(w.position === 1 ? 'It is next in line.' : `It is number ${w.position} in line.`)
      return { line: `${base} This server runs ${w.awakeLimit} at a time.`, notes }
    }
    case 'unavailable':
      if (!virtualDesktop) return { line: msg || base, notes }
      return { line: msg || base, notes, action: { id: mayStart ? 'start' : 'retry', label: 'Retry', ownerOnly: false } }
    default:
      return { line: base, notes }
  }
}

/** What `POST /cli/desktop/start` (or `setup`) refused, in plain words: the
 *  daemon's own sentence where it sends one, plus what the refusal carries
 *  (the desktops that could be closed, Xvfb's last line, the command to run
 *  by hand). */
export function startRefusalText(err: unknown): string {
  const e = (err ?? {}) as { code?: unknown; message?: unknown; body?: unknown }
  const code = typeof e.code === 'string' ? e.code : ''
  const hint = typeof e.message === 'string' ? e.message.trim() : ''
  const body = (e.body && typeof e.body === 'object' ? e.body : {}) as Record<string, unknown>
  const fixed: Record<string, string> = {
    owner_or_admin: 'Starting a desktop is for Owners and Admins.',
    owner_only: 'Only an Owner can set up the desktop.',
    linux_only: 'Linux servers only.',
    desktop_off: 'Remote desktop is off on this server.',
    not_set_up: 'The desktop packages aren’t installed yet.',
    too_many_desktops: 'This server already has as many desktops as it can hold.',
    low_memory: 'This server is low on memory, so K2 didn’t start a desktop.',
    desktop_gone: 'This desktop no longer exists.',
    start_failed: 'The desktop didn’t start.',
    not_virtual: 'This server’s desktop is a real screen session: K2 doesn’t start or close it.',
    agent_desktop_off: AGENT_SWITCH_OFF_LINE,
    no_x11_socket_dir: 'This server’s /tmp/.X11-unix folder is missing or has the wrong permissions.',
    setup_manual: 'K2 can’t install the desktop packages on this server.',
    setup_failed: 'Installing the desktop packages failed.',
    setting_up: 'The desktop packages are already being installed.',
  }
  let line = hint || fixed[code] || 'K2 couldn’t do that.'
  if (code === 'too_many_desktops' && Array.isArray(body.closable) && body.closable.length > 0) {
    line += ` Close one first: ${body.closable.filter((c) => typeof c === 'string').join(', ')}.`
  }
  if (code === 'start_failed' && typeof body.detail === 'string' && body.detail && !line.includes(body.detail)) {
    line += ` ${body.detail}`
  }
  if ((code === 'setup_manual' || code === 'setup_failed') && typeof body.manual === 'string' && body.manual && !line.includes(body.manual)) {
    line += ` To install them by hand: ${body.manual}`
  }
  return line
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
