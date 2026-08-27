// Human-facing catalog of `k2 <noun> …` verbs. Static on purpose — the
// daemon glossary is agent-facing and still says k2so in places.

export interface K2Noun {
  /** CLI noun (`k2 <noun>`). Slash-pairs like `feedback / tickets` stay one row. */
  noun: string
  blurb: string
  example?: string
}

export interface K2NounGroup {
  title: string
  items: K2Noun[]
}

export const K2_CHEAT_SHEET_INTRO =
  'A workspace is an agent. In a K2 terminal you (or the agent) run `k2 <noun> …` to talk, file tickets, and shape the cell.'

export const K2_NOUN_GROUPS: K2NounGroup[] = [
  {
    title: 'Talk',
    items: [
      {
        noun: 'msg',
        blurb: 'Live chat with another workspace agent. Short.',
        example: 'k2 msg sales "ready"',
      },
      {
        noun: 'thread',
        blurb: 'Overlay side channel with the human (not the PTY).',
        example: 'k2 thread sales "hello"',
      },
      {
        noun: 'read',
        blurb: "Peek at another agent's terminal before injecting.",
      },
    ],
  },
  {
    title: 'Inbox & humans',
    items: [
      {
        noun: 'inbox',
        blurb: "This workspace's tray. Others send files with `k2 msg … --inbox-wake`.",
      },
      {
        noun: 'feedback / tickets',
        blurb: 'Durable question for the human (Tickets page).',
      },
    ],
  },
  {
    title: 'Identity',
    items: [
      {
        noun: 'whoami',
        blurb: "This cell's handle/address (`sales/reviewer`).",
      },
      {
        noun: 'connections',
        blurb: 'Linked workspaces (`--users` lists humans on this box).',
      },
    ],
  },
  {
    title: 'Agents & groups',
    items: [
      {
        noun: 'agent',
        blurb: 'Hire / configure / retire; AGENTS.md context.',
      },
      {
        noun: 'preset',
        blurb: 'Which program a workspace launches (Claude, Grok, …).',
      },
      {
        noun: 'project',
        blurb: 'Named group of workspaces + one shared chat.',
      },
      {
        noun: 'workspace',
        blurb: 'List / launch / profile.',
      },
    ],
  },
  {
    title: 'Always-on & more',
    items: [
      { noun: 'heartbeat', blurb: 'Scheduled wakeups.' },
      { noun: 'activity', blurb: 'Audit log.' },
      { noun: 'mail', blurb: 'Real email for agents (not the inbox tray).' },
      { noun: 'wiki', blurb: 'Workspace notes.' },
      { noun: 'skills', blurb: 'Capability profiles.' },
      { noun: 'publish', blurb: 'Expose a local process.' },
      { noun: 'dns', blurb: 'DNS records (Connect, gated).' },
      { noun: 'checkin / done', blurb: 'Heartbeat ping / API-cell complete.' },
    ],
  },
]
