// The Help explorer's friendly layer over the CLIs: which family each
// `k2 <noun>` sits in, a one-line human blurb, and one example. The verbs
// themselves (usage + description) come from the CLI via
// `@shared/cli-catalog.generated.json` (`k2 --schema`), never from here. The
// daemon glossary is agent-facing and still says k2so in places, so the
// blurbs stay here.
//
// Every top-level noun `k2 --schema` lists must appear below (as `noun`,
// `also` or `leftover`); k2Nouns.test.ts fails on a new noun so it gets a
// family and a blurb.

/** `sidecar` = extra chat in this workspace; `linux-sidecar` = Linux process the daemon loads. */
export type K2NounNote = 'sidecar' | 'linux-sidecar'

export interface K2Noun {
  /** CLI noun (`k2 <noun>`). */
  noun: string
  /** Other spellings that run the same thing (`k2 tickets`, `k2 cal`). */
  also?: string[]
  /** Leftover nouns folded into this one; their verbs are not repeated. */
  leftover?: string[]
  blurb: string
  example?: string
  note?: K2NounNote
}

export interface K2NounGroup {
  id: string
  title: string
  items: K2Noun[]
}

export const K2_EXPLORER_INTRO =
  'A workspace is an agent. In a K2 terminal you (or the agent) run `k2 <noun> …`.'

export const K2_NOUN_GROUPS: K2NounGroup[] = [
  {
    id: 'talk',
    title: 'Talk',
    items: [
      { noun: 'msg', blurb: 'Live chat with another workspace agent. Short.', example: 'k2 msg sales "ready"' },
      {
        noun: 'thread',
        blurb: 'Overlay side channel with the human (not the PTY): text, choice cards, secret fields.',
        example: 'k2 thread sales "hello"',
      },
      {
        noun: 'chat',
        blurb: 'Sew chat inside Spaces, as the agent: read, post, reply, DM; community admin with Manage Sew.',
        example: 'k2 chat send hearth/general "deploy done"',
      },
      { noun: 'read', blurb: "Peek at another agent's terminal before injecting.", example: 'k2 read sales --lines 40' },
      { noun: 'talk', blurb: 'Beta: read their screen first, then send, in one call.', example: 'k2 talk sales "ready?"' },
      { noun: 'terminal', blurb: 'Raw bytes into a terminal, or spawn one (advanced).' },
      { noun: 'sessions', blurb: "The daemon's open live sessions for a workspace.", example: 'k2 sessions live sales' },
    ],
  },
  {
    id: 'inbox',
    title: 'Inbox & humans',
    items: [
      {
        noun: 'inbox',
        blurb: "This workspace's tray. Others send files with `k2 msg … --inbox-wake`.",
        example: 'k2 inbox list',
      },
      {
        noun: 'feedback',
        also: ['tickets'],
        blurb: 'Durable question for the human (Tickets page), with an HTML brief.',
        example: 'k2 feedback ask "Ship it?" --options yes,no',
      },
      { noun: 'review', blurb: "Approve, reject or comment on an agent's proposed work.", example: 'k2 review approve sales' },
    ],
  },
  {
    id: 'identity',
    title: 'Identity',
    items: [
      { noun: 'whoami', blurb: "This cell's handle/address (`sales/reviewer`).", example: 'k2 whoami', note: 'sidecar' },
      {
        noun: 'connections',
        blurb: 'Linked workspaces (`--users` lists humans on this box).',
        example: 'k2 connections list --users',
      },
      { noun: 'users', blurb: 'People who can sign in to this server.', example: 'k2 users list' },
    ],
  },
  {
    id: 'agents',
    title: 'Agents & groups',
    items: [
      { noun: 'agent', blurb: 'Hire / configure / retire; AGENTS.md context.', example: 'k2 agent hire ~/ai/sales --name sales' },
      {
        noun: 'clone',
        blurb: 'Clone an agent to another server or folder; agents propose, people approve.',
        example: 'k2 clone preview sales',
      },
      { noun: 'preset', blurb: 'Which program a workspace launches (Claude, Grok, …).', example: 'k2 preset list' },
      {
        noun: 'sidecar',
        blurb: 'Extra chats in a workspace, addressed `<workspace>/<name>`.',
        example: 'k2 sidecar new reviewer --harness claude',
      },
      { noun: 'project', blurb: 'Named group of workspaces + one shared chat.', example: 'k2 project msg launch "done"' },
      { noun: 'workspace', blurb: 'List / launch / profile; pin files in the Files drawer.', example: 'k2 workspace list --running' },
      { noun: 'skills', blurb: 'Capability profiles (SKILL.md).', example: 'k2 skills list' },
      { noun: 'llm', blurb: 'LLM subscriptions and API tokens per tool.', example: 'k2 llm tokens list' },
      { noun: 'usage', blurb: 'Token counts, request log and subscription limits.', example: 'k2 usage tokens --since 7d' },
      { noun: 'hooks', blurb: 'Custom (no-model) agents post to their Thread and report lifecycle events.' },
    ],
  },
  {
    id: 'always-on',
    title: 'Always-on & more',
    items: [
      { noun: 'heartbeat', blurb: 'Scheduled wakeups.', example: 'k2 heartbeat schedule list' },
      {
        noun: 'checkin',
        also: ['done'],
        blurb: 'Heartbeat ping / status; `k2 done` completes an API cell.',
        example: 'k2 checkin --status "halfway"',
      },
      { noun: 'activity', blurb: 'Audit log.', example: 'k2 activity --limit 20' },
      { noun: 'compute', blurb: 'Run builds and tests on your own machines (compute nodes).', example: 'k2 compute nodes' },
      {
        noun: 'sources',
        also: ['source'],
        blurb: 'Checks this server runs for cards and agents, and their triggers.',
        example: 'k2 sources list',
      },
      {
        noun: 'changes',
        blurb: "This workspace's changes, like the Changes tab: diff, stage, discard to Trash, commit.",
        example: 'k2 changes diff',
      },
      {
        noun: 'browser',
        blurb: "Send a Design note about a page to a workspace's Thread and agent.",
        example: 'k2 browser note --url http://localhost:3000 "make the header smaller"',
      },
      { noun: 'commit', blurb: 'AI-assisted commit (`--merge` to merge too).' },
    ],
  },
  {
    id: 'mail',
    title: 'Mail & calendars',
    items: [
      {
        noun: 'mail',
        blurb: 'Hosted addresses for agents (not the inbox tray). Needs the mail sidecar on Linux.',
        example: 'k2 mail wait --subject code',
        note: 'linux-sidecar',
      },
      {
        noun: 'hostmail',
        blurb: 'Hosted mail domains, addresses and deliverability on this server.',
        example: 'k2 hostmail status',
        note: 'linux-sidecar',
      },
      {
        noun: 'calendar',
        also: ['cal'],
        blurb: 'Calendars of a hosted inbox; write events (never sends email).',
        example: 'k2 calendar list',
      },
    ],
  },
  {
    id: 'apps',
    title: 'Apps, Gardens & data',
    items: [
      {
        noun: 'app',
        leftover: ['skin', 'skin-token'],
        blurb: 'Guests, roles, grants and passes for apps on this box. Host the UI with `k2 publish`.',
        example: 'k2 app user list',
      },
      { noun: 'publish', blurb: 'Expose a local process, optionally on a nested URL.', example: 'k2 publish ps' },
      { noun: 'zen', blurb: 'Zen Gardens: gardens, themes, widgets and their data.', example: 'k2 zen guide' },
      { noun: 'store', blurb: "A Garden widget's own data (granted keys only)." },
      { noun: 'db', blurb: "This workspace's loopback Postgres.", example: 'k2 db create' },
      { noun: 'wiki', blurb: 'Workspace notes.', example: 'k2 wiki index' },
    ],
  },
  {
    id: 'network',
    title: 'Network & domains',
    items: [
      { noun: 'dns', blurb: 'DNS records (Connect, gated).', example: 'k2 dns access' },
      { noun: 'domain', blurb: 'Custom domains this server answers for.', example: 'k2 domain list' },
      { noun: 'cert', blurb: 'TLS certificates for attached hostnames.', example: 'k2 cert list' },
      { noun: 'tunnel', blurb: 'The tunnel to your *.k2.dev address.', example: 'k2 tunnel status' },
      { noun: 'connect', blurb: 'Pair a k2.dev subdomain to this device (headless login).', example: 'k2 connect status' },
      { noun: 'remote-session', blurb: 'Owner/admin remote shells behind grants.', example: 'k2 remote-session status' },
    ],
  },
  {
    id: 'server',
    title: 'Server',
    items: [
      { noun: 'daemon', blurb: 'Status, start/stop and logs of the K2 daemon.', example: 'k2 daemon status' },
      { noun: 'settings', blurb: 'Read/update daemon and workspace settings.', example: 'k2 settings' },
      { noun: 'vault', blurb: 'Secrets: fetch by name, never paste.', example: 'k2 vault list' },
      { noun: 'update', blurb: 'Update the K2 app or this CLI.', example: 'k2 update --app' },
      {
        noun: 'hostchat',
        blurb: 'Sew, the chat and voice server, on this Linux box.',
        example: 'k2 hostchat status',
        note: 'linux-sidecar',
      },
      { noun: 'root-helper', blurb: "K2's one root door on Linux: status and update.", example: 'k2 root-helper status' },
      { noun: 'onboarding', blurb: 'First launch: adopt or create workspaces.' },
    ],
  },
  {
    id: 'help',
    title: 'Help & docs',
    items: [
      { noun: 'study', blurb: 'Bounded teaching pages (Pages in this window).', example: 'k2 study errors' },
      { noun: 'glossary', blurb: 'Define a K2 term.', example: 'k2 glossary inbox' },
      { noun: 'help', blurb: 'Daily verbs; `--advanced` for the power-user ones.', example: 'k2 help --advanced' },
      { noun: 'whats-new', blurb: 'Release notes.' },
      { noun: 'version', blurb: 'CLI version.', example: 'k2 version' },
      { noun: '--schema', blurb: 'Every verb as JSON, for agents.', example: 'k2 --schema' },
    ],
  },
]

/** k2-node: one row per subcommand; usage lines come from the generated catalog. */
export const K2_NODE_INTRO =
  'Runs on a compute node (a machine you own). The machine owner installs and manages it; agents use `k2 compute` instead.'

export const K2_NODE_BLURBS: Record<string, string> = {
  setup: 'Install the node (root, once).',
  uninstall: 'Remove the node (root).',
  run: 'Run the node in the foreground.',
  enroll: 'Join a K2 server as one of its compute nodes.',
  request: 'Ask the running node to enroll itself (machine owner).',
  tools: 'Install or list toolchains for jobs (node, python, rust, …).',
  packages: 'Which system packages jobs may install.',
  status: "The node's state.",
  pause: 'Stop taking jobs (now, or after the current one).',
  drain: 'Finish current jobs, then stop taking new ones.',
  resume: 'Take jobs again.',
  policy: "Show or set the node's policy.",
  'install-files': 'Installer: write the service files.',
  'check-home': 'Installer: refuse a home that runs K2.',
  version: 'Node version.',
  admin: 'Remote admin level, audit chain and the root executor.',
}
