// The Help explorer's friendly layer over the CLIs: which family each
// `k2 <noun>` sits in, a one-line human blurb, and one example. The verbs
// themselves (usage + description) come from the CLI via
// `@shared/cli-catalog.generated.json` (`k2 --schema`), never from here. The
// daemon glossary is agent-facing (CLI wording, not a human blurb), so the
// blurbs stay here.
//
// Every top-level noun `k2 --schema` lists must appear below (as `noun`,
// `also` or `leftover`); k2Nouns.test.ts fails on a new noun so it gets a
// family and a blurb.

import { CLI_NAME, PRODUCT_NAME } from '@/lib/brand'

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
  /** A Beta noun: the explorer shows the Beta tag after its name. */
  beta?: boolean
}

export interface K2NounGroup {
  id: string
  title: string
  items: K2Noun[]
}

export const K2_EXPLORER_INTRO =
  `A workspace is an agent. In a ${PRODUCT_NAME} terminal you (or the agent) run \`${CLI_NAME} <noun> …\`.`

export const K2_NOUN_GROUPS: K2NounGroup[] = [
  {
    id: 'talk',
    title: 'Talk',
    items: [
      { noun: 'msg', blurb: 'Live chat with another workspace agent. Short.', example: `${CLI_NAME} msg sales "ready"` },
      {
        noun: 'thread',
        blurb: 'Overlay side channel with the human (not the PTY): text, choice cards, secret fields.',
        example: `${CLI_NAME} thread sales "hello"`,
      },
      {
        noun: 'chat',
        blurb: 'Chat inside Spaces, as the agent: read, post, reply, DM; community admin with Manage chat and voice.',
        example: `${CLI_NAME} chat send hearth/general "deploy done"`,
      },
      {
        noun: 'voice',
        blurb: `Preview: voice rooms on this server. See who is in them; the owner and Admins change and moderate them. People talk from the ${PRODUCT_NAME} app.`,
        example: `${CLI_NAME} voice rooms`,
      },
      {
        noun: 'postal',
        blurb: 'Friends: agents on other servers this agent can talk to, through the Postal service (handle::label.postal.bot). Every step is a card your person answers.',
        example: `${CLI_NAME} postal status`,
        beta: true,
      },
      { noun: 'read', blurb: "Peek at another agent's terminal before injecting.", example: `${CLI_NAME} read sales --lines 40` },
      { noun: 'talk', blurb: 'Read their screen first, then send, in one call.', example: `${CLI_NAME} talk sales "ready?"`, beta: true },
      { noun: 'terminal', blurb: 'Raw bytes into a terminal, or spawn one (advanced).' },
      { noun: 'sessions', blurb: 'Who is working in this workspace, and past conversations.', example: `${CLI_NAME} sessions --past --search scroll` },
    ],
  },
  {
    id: 'inbox',
    title: 'Inbox & humans',
    items: [
      {
        noun: 'inbox',
        blurb: `This workspace's tray. Others send files with \`${CLI_NAME} msg … --inbox-wake\`.`,
        example: `${CLI_NAME} inbox list`,
      },
      {
        noun: 'done',
        blurb: 'Mark your task done (or blocked); in an API cell it completes the session.',
        example: `${CLI_NAME} done --blocked "waiting on review"`,
      },
      {
        noun: 'respond',
        blurb: 'In a session the API started: send a line back to the API caller (`--final` for the last).',
        example: `${CLI_NAME} respond --final "report written to out/report.md"`,
      },
      {
        noun: 'feedback',
        also: ['tickets'],
        blurb: 'Durable question for the human (Tickets page), with an HTML brief.',
        example: `${CLI_NAME} feedback ask "Ship it?" --options yes,no`,
      },
      { noun: 'review', blurb: "Approve, reject or comment on an agent's proposed work.", example: `${CLI_NAME} review approve sales` },
      {
        noun: 'approve',
        blurb: `Ask a person to approve exact bytes on their phone (Face ID); they sign there, ${PRODUCT_NAME} only carries it.`,
        example: `${CLI_NAME} approve request --payload p.bin --wait --out approval.json`,
      },
      {
        noun: 'notify',
        blurb: 'Email a person on this server, once the owner turned it on for this workspace (text only).',
        example: `${CLI_NAME} notify email owner --subject "Build done" --text "All green."`,
      },
    ],
  },
  {
    id: 'identity',
    title: 'Identity',
    items: [
      { noun: 'whoami', blurb: "This cell's handle/address (`sales/reviewer`).", example: `${CLI_NAME} whoami`, note: 'sidecar' },
      {
        noun: 'connections',
        blurb: 'Linked workspaces (`--users` lists humans on this box).',
        example: `${CLI_NAME} connections list --users`,
      },
      { noun: 'users', blurb: 'People who can sign in to this server.', example: `${CLI_NAME} users list` },
    ],
  },
  {
    id: 'agents',
    title: 'Agents & groups',
    items: [
      { noun: 'agent', blurb: 'Hire / configure / retire; AGENTS.md context.', example: `${CLI_NAME} agent hire ~/ai/sales --name sales` },
      {
        noun: 'clone',
        blurb: 'Clone an agent to another server or folder; agents propose, people approve.',
        example: `${CLI_NAME} clone preview sales`,
      },
      { noun: 'preset', blurb: 'Which program a workspace launches (Claude, Grok, …).', example: `${CLI_NAME} preset list` },
      {
        noun: 'sidecar',
        blurb: 'Extra chats in a workspace, addressed `<workspace>/<name>`.',
        example: `${CLI_NAME} sidecar new reviewer --harness claude`,
      },
      { noun: 'project', blurb: 'Named group of workspaces + one shared chat.', example: `${CLI_NAME} project msg launch "done"` },
      {
        noun: 'focus-group',
        blurb: "The sidebar's focus groups of workspaces; an agent asks on a card to change one.",
        example: `${CLI_NAME} focus-group list`,
      },
      {
        noun: 'organization',
        blurb: "The Org Chart: orgs, heads and the daily bulletin; reach a department's head.",
        example: `${CLI_NAME} organization bulletin`,
      },
      { noun: 'workspace', blurb: 'List / launch / profile; pin files in the Files drawer.', example: `${CLI_NAME} workspace list --running` },
      { noun: 'skills', blurb: 'Capability profiles (SKILL.md).', example: `${CLI_NAME} skills list` },
      { noun: 'llm', blurb: 'LLM subscriptions and API tokens per tool.', example: `${CLI_NAME} llm tokens list` },
      { noun: 'usage', blurb: 'Token counts, request log and subscription limits.', example: `${CLI_NAME} usage tokens --since 7d` },
      { noun: 'hooks', blurb: 'Custom (no-model) agents post to their Thread and report lifecycle events.' },
    ],
  },
  {
    id: 'always-on',
    title: 'Always-on & more',
    items: [
      { noun: 'heartbeat', blurb: 'Scheduled wakeups.', example: `${CLI_NAME} heartbeat schedule list` },
      {
        noun: 'remind',
        also: ['reminders'],
        blurb: 'A one-time wake: it fires once, then goes away.',
        example: `${CLI_NAME} remind "call Jane back" --in 45m`,
      },
      { noun: 'activity', blurb: 'Audit log.', example: `${CLI_NAME} activity --limit 20` },
      { noun: 'compute', blurb: 'Run builds and tests on your own machines (compute nodes).', example: `${CLI_NAME} compute nodes` },
      { noun: 'ci', blurb: 'CI on compute: check every push, post GitHub statuses.', example: `${CLI_NAME} ci status` },
      {
        noun: 'sources',
        also: ['source'],
        blurb: 'Checks this server runs for cards and agents, and their triggers.',
        example: `${CLI_NAME} sources list`,
      },
      {
        noun: 'changes',
        blurb: "This workspace's changes, like the Changes tab: diff, stage, discard to Trash, commit.",
        example: `${CLI_NAME} changes diff`,
      },
      {
        noun: 'browser',
        blurb: "Send a Design note about a page to a workspace's Thread and agent.",
        example: `${CLI_NAME} browser note --url http://localhost:3000 "make the header smaller"`,
      },
      {
        noun: 'sim',
        blurb: "Drive this workspace's phone device (the Mobile tab's): read the screen, tap, type, install.",
        example: `${CLI_NAME} sim tree`,
      },
      {
        noun: 'desktop',
        blurb: `See and control this computer's screen from another ${PRODUCT_NAME} window (Owners and Admins; off until turned on).`,
        example: `${CLI_NAME} desktop status`,
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
        example: `${CLI_NAME} mail wait --subject code`,
        note: 'linux-sidecar',
      },
      {
        noun: 'hostmail',
        blurb: 'Hosted mail domains, addresses and deliverability on this server.',
        example: `${CLI_NAME} hostmail status`,
        note: 'linux-sidecar',
      },
      {
        noun: 'calendar',
        also: ['cal'],
        blurb: 'Calendars of a hosted inbox; write events (never sends email).',
        example: `${CLI_NAME} calendar list`,
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
        blurb: `Guests, roles, grants and passes for apps on this box. Host the UI with \`${CLI_NAME} publish\`.`,
        example: `${CLI_NAME} app user list`,
      },
      {
        noun: 'capabilities',
        blurb: 'What this server can do for Apps before you try; each "no" names the fix.',
        example: `${CLI_NAME} capabilities`,
      },
      { noun: 'publish', blurb: 'Expose a local process, optionally on a nested URL.', example: `${CLI_NAME} publish ps` },
      { noun: 'zen', blurb: 'Zen Gardens: gardens, themes, widgets and their data.', example: `${CLI_NAME} zen guide` },
      { noun: 'store', blurb: "A Garden widget's own data (granted keys only)." },
      {
        noun: 'board',
        blurb: "The Board Garden's whiteboards, for an agent with a grant on the board: read and change them by Board's own rules.",
        example: `${CLI_NAME} board boards`,
      },
      { noun: 'db', blurb: "This workspace's loopback Postgres.", example: `${CLI_NAME} db create` },
      {
        noun: 'doc',
        also: ['docs'],
        blurb: 'Live docs: edit the same file with people and other agents, with presence, your own undo and history.',
        example: `${CLI_NAME} doc grep -n TODO notes.md`,
      },
      { noun: 'wiki', blurb: 'Workspace notes.', example: `${CLI_NAME} wiki index` },
    ],
  },
  {
    id: 'network',
    title: 'Network & domains',
    items: [
      { noun: 'dns', blurb: 'DNS records (Connect, gated).', example: `${CLI_NAME} dns access` },
      { noun: 'domain', blurb: 'Custom domains this server answers for.', example: `${CLI_NAME} domain list` },
      { noun: 'cert', blurb: 'TLS certificates for attached hostnames.', example: `${CLI_NAME} cert list` },
      { noun: 'firewall', blurb: 'The ports this server serves publicly, who applies the list, and the App guard.', example: `${CLI_NAME} firewall` },
      { noun: 'front', blurb: `${PRODUCT_NAME}'s front on this box's port 443: passes each name, still encrypted, to its server (Linux).`, example: `${CLI_NAME} front status` },
      { noun: 'tunnel', blurb: 'The tunnel to your *.k2.dev address.', example: `${CLI_NAME} tunnel status` },
      { noun: 'connect', blurb: 'Pair a k2.dev subdomain to this device (headless login).', example: `${CLI_NAME} connect status` },
      { noun: 'remote', blurb: `Sign this computer out of another ${PRODUCT_NAME} server.`, example: `${CLI_NAME} remote logout --host <server>` },
      { noun: 'remote-session', blurb: 'Owner/admin remote shells behind grants.', example: `${CLI_NAME} remote-session status` },
      { noun: 'fed', blurb: `Connect agents across servers: pair with another ${PRODUCT_NAME} server so agents can message across.`, example: `${CLI_NAME} fed peers` },
    ],
  },
  {
    id: 'server',
    title: 'Server',
    items: [
      { noun: 'daemon', blurb: `Status, start/stop and logs of the ${PRODUCT_NAME} daemon.`, example: `${CLI_NAME} daemon status` },
      { noun: 'settings', blurb: 'Read/update daemon and workspace settings.', example: `${CLI_NAME} settings` },
      { noun: 'setup', blurb: 'Your setup list on this server: what is done, what is left.', example: `${CLI_NAME} setup` },
      { noun: 'push', blurb: "The phones that get this server's pushes, and encrypted pushes.", example: `${CLI_NAME} push devices` },
      { noun: 'notifications', blurb: `The senders ${PRODUCT_NAME}'s notification mail leaves from, and your notice address.`, example: `${CLI_NAME} notifications email` },
      { noun: 'api-key', blurb: 'Keys for the public /v1 API (owner only): which workspaces and doors each opens.', example: `${CLI_NAME} api-key list` },
      { noun: 'migrate', blurb: "Move this box's vault to new hardware as one encrypted file.", example: `${CLI_NAME} migrate status` },
      { noun: 'vault', blurb: 'Secrets: fetch by name, never paste.', example: `${CLI_NAME} vault list` },
      { noun: 'job', blurb: 'Scripts on a schedule, run by the daemon with their own secrets.', example: `${CLI_NAME} job list` },
      { noun: 'update', blurb: 'Check for desktop app updates (the CLI updates with the app).', example: `${CLI_NAME} update --app` },
      {
        noun: 'hostchat',
        blurb: 'Chat and voice on this Linux server: the chat and voice server, powered by Sew.',
        example: `${CLI_NAME} hostchat status`,
        note: 'linux-sidecar',
      },
      { noun: 'root-helper', blurb: `${PRODUCT_NAME}'s one root door on Linux: status and update.`, example: `${CLI_NAME} root-helper status` },
      { noun: 'git-tools', blurb: "Is git usable here: install Apple's command line tools, or how to install git on Linux.", example: `${CLI_NAME} git-tools` },
      { noun: 'onboarding', blurb: 'First launch: adopt or create workspaces.' },
    ],
  },
  {
    id: 'help',
    title: 'Help & docs',
    items: [
      { noun: 'study', blurb: 'Bounded teaching pages (Pages in this window).', example: `${CLI_NAME} study errors` },
      { noun: 'explain', blurb: 'What an App refusal id means, its fix and who fixes it.', example: `${CLI_NAME} explain apps_manage_off` },
      {
        noun: 'send-feedback',
        blurb: `Tell Alakazam Labs, who make ${PRODUCT_NAME}, about ${PRODUCT_NAME} itself: a bug or an idea (not your human: that is tickets).`,
        example: `${CLI_NAME} send-feedback "<text>" --category bug`,
      },
      { noun: 'glossary', blurb: `Define a ${PRODUCT_NAME} term.`, example: `${CLI_NAME} glossary inbox` },
      { noun: 'help', blurb: 'Daily verbs; `--advanced` for the power-user ones.', example: `${CLI_NAME} help --advanced` },
      { noun: 'whats-new', blurb: 'Release notes.' },
      { noun: 'version', blurb: 'CLI version.', example: `${CLI_NAME} version` },
      {
        noun: 'cli',
        blurb: 'The k5 binary only: which command families it answers itself, and how often the rest fall through.',
        example: `${CLI_NAME} cli status`,
        beta: true,
      },
      { noun: '--schema', blurb: 'Every verb as JSON, for agents.', example: `${CLI_NAME} --schema` },
    ],
  },
]

/** k2-node: one row per subcommand; usage lines come from the generated catalog. */
export const K2_NODE_INTRO =
  `Runs on a compute node (a machine you own). The machine owner installs and manages it; agents use \`${CLI_NAME} compute\` instead.`

export const K2_NODE_BLURBS: Record<string, string> = {
  setup: 'Install the node (root, once).',
  uninstall: 'Remove the node (root).',
  run: 'Run the node in the foreground.',
  enroll: `Join a ${PRODUCT_NAME} server as one of its compute nodes.`,
  request: 'Ask the running node to enroll itself (machine owner).',
  tools: 'Install or list toolchains for jobs (node, python, rust, …).',
  packages: 'Which system packages jobs may install.',
  status: "The node's state.",
  pause: 'Stop taking jobs (now, or after the current one).',
  drain: 'Finish current jobs, then stop taking new ones.',
  resume: 'Take jobs again.',
  policy: "Show or set the node's policy.",
  'install-files': 'Installer: write the service files.',
  'check-home': `Installer: refuse a home that runs ${PRODUCT_NAME}.`,
  version: 'Node version.',
  admin: 'Remote admin level, audit chain and the root executor.',
  servers: `The ${PRODUCT_NAME} servers this node serves: add, limits, pause, revoke (node sharing).`,
  sim: "iPhones on this Mac (the iOS Simulator) for one of your own servers: turn on or off, or see what's set up.",
  gc: "Free disk now: ended jobs' folders and idle build caches (machine owner).",
  'wall-probe': 'Tests only: what a walled job on this node can reach.',
}
