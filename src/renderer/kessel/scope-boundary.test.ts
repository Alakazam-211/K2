// Home M1 — MS79: the "required scope" rule is enforced at module
// boundaries with two ratchets over the renderer source.
//
// 1. Raw daemon transport. `fetch(` and `new WebSocket(` may only appear in
//    the files listed in RAW_TRANSPORT_ALLOWLIST: the scoped request layer
//    itself, and the call sites that predate it (each one resolves creds
//    through `getDaemonWs(scope)` or is not a daemon call at all). A new
//    raw call anywhere else fails this test. Move it into the scoped layer,
//    or add it here with a reason.
// 2. `primaryScope()` callers. In M1 every caller passes the window's
//    primary scope. PRIMARY_SCOPE_CALLERS is the exact list of non-test
//    files that call it. A NEW file fails the test (take a scope argument
//    instead, or add it deliberately); M3/M4 shrink the list as room code
//    takes its scope from the room. A file that stops calling it must also
//    be removed from the list, so the list never goes stale.

import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..')

function walk(dir: string, out: string[]): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (/\.tsx?$/.test(e.name)) out.push(p)
  }
  return out
}

const SOURCE_FILES = walk(RENDERER, [])
  .map((f) => relative(RENDERER, f).split(sep).join('/'))
  .filter((f) => !/\.(ms)?test\.tsx?$/.test(f) && !f.startsWith('test-utils/'))
  .sort()

/** Source with block and line comments removed (crude but enough here:
 *  a `//` only starts a comment at line start or after whitespace). */
function read(f: string): string {
  return readFileSync(join(RENDERER, f), 'utf8')
    .replace(/(^|\s)\/\*[\s\S]*?\*\//g, '$1')
    .replace(/(^|\s)\/\/.*$/gm, '$1')
}

const RAW_TRANSPORT = /(?<![\w.$])fetch\(|new WebSocket\(/

/** file → why a raw `fetch(` / `new WebSocket(` is allowed there. */
const RAW_TRANSPORT_ALLOWLIST: Record<string, string> = {
  // The scoped layer.
  'lib/daemon-cli.ts': 'the scoped /cli/* client (cliFetch, localDaemonCliPost)',
  // MS70: every daemon socket (grid, events, overlay, transcript, chatter)
  // dials through the per-server queue, so no other file constructs one.
  'lib/grid-dial-queue.ts': 'the per-server dial queue for every daemon socket',
  // Window-level connection plumbing (any saved host, by its own creds).
  'components/ConnectionGate.tsx': 'boot-status / session probes for the window host',
  'lib/host-ops.ts': 'per-saved-host ops with that host\'s own creds',
  'lib/host-pool-instance.ts': 'the connection pool\'s status checks and sign-out, each server\'s own creds',
  'lib/remote-session.ts': 'whoami + login for one host id',
  'stores/connect-host.ts': 'login for one saved host',
  'lib/connect-validate.ts': 'add-server validation probe',
  'lib/password-rotation.ts': 'password rotation for one host',
  'web/session-token.ts': 'hosted-web cookie session',
  // Pre-M1 raw daemon calls that resolve creds through getDaemonWs(scope).
  'stores/tabs.ts': 'closeV2Session(scope): v2/close on the room\'s own scope (M3, MS42)',
  'kessel-term/TerminalPane.tsx': 'v2 spawn (M4 gives the pane a scope prop)',
  'components/Settings/sections/access-audit-api.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'components/Settings/sections/CompanionSection.tsx': 'raw /cli fetch via getDaemonWs(scope)',
  'components/Settings/shared/phone-pairing-api.ts': 'raw /cli fetch via getDaemonWs(scope) (its 401 password_invalid must not reach the shared re-auth path)',
  'components/Settings/sections/DomainsSection.tsx': 'raw /cli fetch via getDaemonWs(scope)',
  'components/Settings/sections/K2ConnectSection.tsx': 'raw /cli fetch via getDaemonWs(scope)',
  'hooks/useTunnelUrls.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'lib/federation.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'lib/llmDaemonClient.ts': 'assistant LLM, window server (MS22)',
  'lib/daemon-settings.ts': 'settings fetch via getDaemonWs(scope)',
  'stores/claude-auth.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'lib/app-update-driver.ts': 'local app-update driver',
  // 0.46.7 live docs (plan L2): the Live view's one-attempt socket to
  // WS /cli/docs/live; the URL and token come from the caller's scope, and
  // the editor's own retry loop owns reconnects (not the dial queue yet).
  // Temporary: the Live editor slice must dial through grid-dial-queue
  // (MS70) before any UI uses it.
  'lib/docs-live-transport.ts': 'live docs socket (WS /cli/docs/live) from the caller\'s scope; one attempt per connect',
  // Not daemon calls.
  'components/Settings/lib/k2-account.ts': 'Supabase account API, not a daemon',
  'components/Settings/lib/account-db-daemon.ts': 'raw /cli/tunnel/config fetch via getLocalDaemonWs() (the account session is this machine\'s, like the Tunnel panel)',
  // 0.45.4 compute: the K2 account's enroll-ticket endpoint (k2.dev), not a daemon.
  'components/Settings/lib/compute-account.ts': 'K2 account enroll-ticket API, not a daemon',
  'components/Settings/CustomThemeCreator.tsx': 'example code inside a prompt string',
  'components/Settings/sections/CodeEditorSettingsSection.tsx': 'example code inside a prompt string',
  'hooks/useGit.ts': 'a local function named fetch (git refresh)',
  'dev/room-frame-probe.ts': 'dev-only P1.5 probe, not shipped',
  'lib/zen/zen-lib-loader.ts':
    "Zen widget library: reads the bundled files from the app's own origin (/zen-lib/), not a daemon call; downloads go through daemonCli on the local scope",
  'dev/zen-spike-s0.ts':
    'Zen v2 S0 spike (VITE_K2_ZEN_SPIKE=s0 builds only): reads its own page CSP header; the rest are probes inside the sealed frame',
}

const PRIMARY_SCOPE_CALLERS: readonly string[] = [
  'App.tsx',
  'components/AIFileEditor/AIFileEditor.tsx',
  'components/AgentPersonaEditor/AgentPersonaEditor.tsx',
  'components/AgentsPanel/AgentsPanel.tsx',
  'components/ConnectionGate.tsx',
  'components/Feedback/FeedbackItemView.tsx',
  // Home M4: window-level surfaces about the window's own server.
  'components/FocusWindow/FocusWorkspaceHeader.tsx',
  'components/Feedback/FeedbackPage.tsx',
  // Tickets live on the window's own server: the chat rail and "me".
  'components/Feedback/TicketAgentRail.tsx',
  'components/Feedback/feedback-api.ts',
  'components/Feedback/useTicketMe.ts',
  'components/FileViewerPane/CodeEditor.tsx',
  'components/GitInitDialog/GitInitDialog.tsx',
  'components/GrokBots/GrokBotRows.tsx',
  'components/HeartbeatScheduleDialog/HeartbeatScheduleDialog.tsx',
  // 0.46.5 Mark all as read: a Home row on the window's own server reads and
  // marks through it (a row on another server goes through a room scope).
  'components/Home/HomeSidebar.tsx',
  'components/MergeDialog/MergeDialog.tsx',
  // 0.46.0: the "Your account" setup prompt is about the window's own server.
  'components/OwnerAccountPrompt/OwnerAccountPrompt.tsx',
  'components/Presence/PresenceKickButton.tsx',
  'components/Projects/ProjectChatPanel.tsx',
  'components/Projects/ProjectDashboard.tsx',
  // 0.46.1: the shared icon cache's single-flight GET (was ProjectGroupAvatar).
  'components/Projects/group-icon-cache.ts',
  'components/Projects/ProjectNav.tsx',
  'components/Projects/ProjectSettings.tsx',
  'components/Projects/ProjectsPage.tsx',
  'components/RemoteFolderPicker/RemoteFolderPicker.tsx',
  'components/RenameAgentDialog/RenameAgentDialog.tsx',
  'components/RunningAgentsPanel/RunningAgentsPanel.tsx',
  'components/Settings/ContextCatalogCreator.tsx',
  'components/Settings/CustomThemeCreator.tsx',
  'components/Settings/DisableWorktreesDialog.tsx',
  'components/Settings/sections/AgentsSection.tsx',
  'components/Settings/sections/ApiTokensSection.tsx',
  'components/Settings/sections/api-keys-api.ts',
  // 0.46.5 App phone pairing: an App's phone-app config is about the
  // window's own server (Settings), like app-people-api below.
  'components/Settings/sections/app-pairing-api.ts',
  // 0.46.3 App mobile access: the App's access list, role templates and
  // device sign-in settings are about the window's own server (Settings).
  'components/Settings/sections/app-people-api.ts',
  // 0.46.4 box usage (disk guard Slice B): Settings → Apps → This server's
  // disk bars, trends and clean are about the window's own server.
  'components/Settings/sections/app-resources-api.ts',
  // 0.46.7 Releases: an App's update channels and releases are about the
  // window's own server (Settings).
  'components/Settings/sections/app-updates-api.ts',
  'components/Settings/sections/CanonicalAgentButtons.tsx',
  'components/Settings/sections/CanonicalAgentModal.tsx',
  'components/Settings/sections/CompanionSection.tsx',
  'components/Settings/sections/ContextLayersPreview.tsx',
  'components/Settings/sections/ContextStackEditor.tsx',
  'components/Settings/sections/DomainsSection.tsx',
  'components/Settings/sections/FanoutConfirmModal.tsx',
  'components/Settings/sections/GeneralSection.tsx',
  'components/Settings/sections/HeartbeatsSection.tsx',
  'components/Settings/sections/K2ConnectSection.tsx',
  'components/Settings/sections/LocalGrokBotsSection.tsx',
  'components/Settings/sections/PeopleSection.tsx',
  'components/Settings/sections/ProjectsSection.tsx',
  'components/Settings/sections/RoleSkillEditor.tsx',
  'components/Settings/sections/SkinAccessSection.tsx',
  'components/Settings/sections/TunnelUrlsPanel.tsx',
  'components/Settings/sections/UserTemplatesSection.tsx',
  'components/Settings/sections/WakeSchedulerSection.tsx',
  // 0.46.0: Settings → General → Your account reads this server's whoami/users.
  'components/Settings/sections/YourAccountRow.tsx',
  'components/Settings/sections/access-audit-api.ts',
  // 0.46.3: Settings → Apps → This server reads and prepares the window's
  // own server (the box doctor, prepare, engine install).
  'components/Settings/sections/apps-server-api.ts',
  // 0.46.3: an agent's App switches and cores (its workspace card) and
  // Settings → Apps' rows and review note write the window's own server.
  'components/Settings/sections/app-access-api.ts',
  // 0.45.6 Garden v3: Settings → External Data lists this server's sources.
  'components/Settings/sections/DataSourcesSection.tsx',
  'components/Settings/sections/data-sources-ui.ts',
  // 0.46.3: Settings → Gardens matches each Garden's source addresses to
  // the sources of the window's server (the External Data list above).
  'components/Settings/sections/ZenGardensAccessPage.tsx',
  'components/Settings/shared/AgentAccountsColumn.tsx',
  'components/Settings/sections/data-api.ts',
  'components/Settings/sections/email-api.ts',
  // Settings → Vault is the window's server, like the other Settings pages.
  'components/Settings/sections/vault-api.ts',
  // 0.45.4 Settings → Tickets: the window's server's ticket rules.
  'components/Settings/sections/tickets-settings-api.ts',
  // 0.45.4 Settings → Compute: the window's server's nodes and grants.
  'components/Settings/sections/compute-api.ts',
  // 0.45.5 Settings → Compute approvals (node admin S10): the window's
  // server's held admin requests.
  'components/Settings/sections/compute-admin-api.ts',
  // 0.46.3 Settings → Access → Permission Cards (Beta): the window's
  // server's switches, agents and requests waiting on you.
  'components/Settings/sections/permission-cards-api.ts',
  // S32 K27 Settings → Chat Hosting: the window's server's Sew install, and
  // its `sew_changed` refetch event.
  'components/Settings/sections/sew-api.ts',
  'components/Settings/sections/SewSection.tsx',
  // 0.45.4 federation: peer-list change events for the window's server.
  'components/Settings/sections/ConnectionsSection.tsx',
  // 0.45.5 Settings → Notifications (Thread push): the window's server's
  // push prefs, its `thread-push-v1` feature, and its refetch event.
  'components/Settings/sections/NotificationsSection.tsx',
  'components/Settings/sections/notify-api.ts',
  'components/Settings/sections/useNotifyPrefs.ts',
  // 0.46.1 Settings → App Approvals (door S20): the window's server's held
  // App requests.
  'components/Settings/sections/app-approvals-api.ts',
  // 0.46.1 secret handoff (H4): Vault Incoming/Sent default to the window's
  // server; callers with a room pass its scope.
  'components/Settings/sections/handoff-api.ts',
  // 0.46.7 outside forwards (FW4): Email Hosting → a mailbox → Forwarding
  // is about the window's own server's mail; callers with a room pass its
  // scope.
  'components/Settings/sections/mail-forward-api.ts',
  // 0.46.7 multiplayer cores: Settings → Cores → Multiplayer switches the
  // window's own server's doc cores; callers with a room pass its scope.
  'components/Settings/sections/MultiplayerSection.tsx',
  'components/Settings/sections/multiplayer-cores-api.ts',
  // 0.46.1 remote switch: the per-agent switch in the window's server's
  // workspace settings.
  'components/Settings/shared/RemoteInstructToggle.tsx',
  'components/Sidebar/ActiveBar.tsx',
  'components/Sidebar/IconRail.tsx',
  'components/Sidebar/ProjectAvatar.tsx',
  'components/Sidebar/Sidebar.tsx',
  'components/Sidebar/WorktreeDialog.tsx',
  'components/TopBar/TopBar.tsx',
  // 0.43.2 Z14: the top bar's window-server target (a focused room uses
  // the room's scope).
  'components/TopBar/top-bar-scope.ts',
  'components/WhatsNewModal/WhatsNewModal.tsx',
  'components/Wiki/wiki-api.ts',
  'components/WorkspaceAssistant/AssistantBar.tsx',
  'components/WorkspacePanel/WorkspaceCompletionSoundToggle.tsx',
  'components/common/HeartbeatSessionPicker.tsx',
  'hooks/useCursorMigrationCheck.ts',
  'lib/context-stack.ts',
  'lib/daemon-settings.ts',
  'lib/external-drop-router.ts',
  'lib/federation.ts',
  // 0.46.4 first run without git: whether git runs on the window's own
  // server (Settings → General row, Add Workspace / Git Init banners).
  'lib/git-tools.ts',
  'lib/heartbeat-delivery.ts',
  'lib/llmDaemonClient.ts',
  'lib/pick-remote-image.ts',
  'lib/start-clone-to.ts',
  // 0.45.4: the landing-tab pref is keyed by the window's server + login.
  'lib/ticket-open-pref.ts',
  // Home M3 / Q4: the assistant acts only on rooms on the window's server.
  'lib/workspace-ops-router.ts',
  'stores/active-agents.ts',
  // 0.46.4 Settings → App Approvals badge: the window's server's held App
  // requests (its push on that server's events bus; a server switch reloads).
  'stores/app-approvals.ts',
  'stores/assistant.ts',
  'stores/claude-auth.ts',
  'stores/custom-themes.ts',
  'stores/feedback.ts',
  'stores/focus-groups.ts',
  'stores/grokbots.ts',
  'stores/heartbeat-sessions.ts',
  // Heartbeat S6: Keep awake is a server's machine. 0.43.2 Z17: the
  // window's own entry; a focused room's entry uses the room's scope.
  'stores/keep-awake.ts',
  // 0.45.1: a server switch drops the window's token list and reloads it
  // from the new server (Settings → LLMs is per server).
  'stores/llm-accounts.ts',
  'stores/presence.ts',
  'stores/presets.ts',
  'stores/project-groups.ts',
  'stores/projects.ts',
  // Home M3: the primary room is built on the primary scope.
  'stores/room.ts',
  'stores/subscription-usage.ts',
  'stores/tabs.ts',
  'stores/timer.ts',
  // thread-read-v1: the window server's per-person unread counts; a server
  // switch resets and refetches (another server's rows read their own).
  'stores/unread.ts',
  // Usage v2: a server switch drops the window's limits doc.
  'stores/usage-limits.ts',
]

describe('MS79 scope boundary ratchets', () => {
  it('raw fetch( / new WebSocket( appear only in the allowlisted files', () => {
    const found = SOURCE_FILES.filter((f) => RAW_TRANSPORT.test(read(f)))
    const unexpected = found.filter((f) => !(f in RAW_TRANSPORT_ALLOWLIST))
    const stale = Object.keys(RAW_TRANSPORT_ALLOWLIST).filter((f) => !found.includes(f))
    expect(unexpected).toEqual([])
    expect(stale).toEqual([])
  })

  it('primaryScope() is called only by the listed files', () => {
    const found = SOURCE_FILES.filter(
      (f) => f !== 'kessel/server-scope.ts' && /\bprimaryScope\(\)/.test(read(f)),
    )
    const unexpected = found.filter((f) => !PRIMARY_SCOPE_CALLERS.includes(f))
    const stale = PRIMARY_SCOPE_CALLERS.filter((f) => !found.includes(f))
    expect(unexpected).toEqual([])
    expect(stale).toEqual([])
  })

  it('the request layer itself never resolves primaryScope()', () => {
    for (const f of [
      'lib/daemon-cli.ts',
      'kessel/daemon-ws.ts',
      'lib/grid-dial-queue.ts',
      'lib/terminal-daemon.ts',
      'lib/upload-to-remote.ts',
      'lib/clone-to.ts',
      'lib/clone-pull.ts',
      'lib/handle-remote-drop.ts',
      'components/Projects/projects-api.ts',
      'components/SessionView/useOverlayThread.ts',
      'components/SessionView/useOverlayChatter.ts',
    ]) {
      expect([f, /\bprimaryScope\(\)/.test(read(f))]).toEqual([f, false])
    }
  })
})
