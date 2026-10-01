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
  'components/Settings/sections/DomainsSection.tsx': 'raw /cli fetch via getDaemonWs(scope)',
  'components/Settings/sections/K2ConnectSection.tsx': 'raw /cli fetch via getDaemonWs(scope)',
  'hooks/useTunnelUrls.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'lib/federation.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'lib/llmDaemonClient.ts': 'assistant LLM, window server (MS22)',
  'lib/daemon-settings.ts': 'settings fetch via getDaemonWs(scope)',
  'stores/claude-auth.ts': 'raw /cli fetch via getDaemonWs(scope)',
  'lib/app-update-driver.ts': 'local app-update driver',
  // Not daemon calls.
  'components/Settings/lib/k2-account.ts': 'Supabase account API, not a daemon',
  'components/Settings/CustomThemeCreator.tsx': 'example code inside a prompt string',
  'components/Settings/sections/CodeEditorSettingsSection.tsx': 'example code inside a prompt string',
  'hooks/useGit.ts': 'a local function named fetch (git refresh)',
  'dev/room-frame-probe.ts': 'dev-only P1.5 probe, not shipped',
}

const PRIMARY_SCOPE_CALLERS: readonly string[] = [
  'App.tsx',
  'components/AIFileEditor/AIFileEditor.tsx',
  'components/AgentPane/InboxChatAboutDialog.tsx',
  'components/AgentPersonaEditor/AgentPersonaEditor.tsx',
  'components/AgentsPanel/AgentsPanel.tsx',
  'components/ConnectionGate.tsx',
  'components/Feedback/FeedbackItemView.tsx',
  'components/Feedback/FeedbackPage.tsx',
  'components/Feedback/feedback-api.ts',
  'components/FileViewerPane/CodeEditor.tsx',
  'components/GitInitDialog/GitInitDialog.tsx',
  'components/HeartbeatScheduleDialog/HeartbeatScheduleDialog.tsx',
  'components/MergeDialog/MergeDialog.tsx',
  'components/Presence/PresenceKickButton.tsx',
  'components/Projects/ProjectChatPanel.tsx',
  'components/Projects/ProjectDashboard.tsx',
  'components/Projects/ProjectGroupAvatar.tsx',
  'components/Projects/ProjectNav.tsx',
  'components/Projects/ProjectSettings.tsx',
  'components/Projects/ProjectsPage.tsx',
  'components/RemoteFolderPicker/RemoteFolderPicker.tsx',
  'components/RunningAgentsPanel/RunningAgentsPanel.tsx',
  'components/SessionView/ChatOverlayPane.tsx',
  'components/SessionView/ChatterOverlayPane.tsx',
  'components/SessionView/ThreadOverlayPane.tsx',
  'components/Settings/ContextCatalogCreator.tsx',
  'components/Settings/CustomThemeCreator.tsx',
  'components/Settings/DisableWorktreesDialog.tsx',
  'components/Settings/sections/AgentsSection.tsx',
  'components/Settings/sections/ApiTokensSection.tsx',
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
  'components/Settings/sections/PeopleSection.tsx',
  'components/Settings/sections/ProjectsSection.tsx',
  'components/Settings/sections/RoleSkillEditor.tsx',
  'components/Settings/sections/SkinAccessSection.tsx',
  'components/Settings/sections/TokenUsageSection.tsx',
  'components/Settings/sections/UserTemplatesSection.tsx',
  'components/Settings/sections/WakeSchedulerSection.tsx',
  'components/Settings/sections/access-audit-api.ts',
  'components/Settings/sections/data-api.ts',
  'components/Settings/sections/email-api.ts',
  'components/Sidebar/ActiveBar.tsx',
  'components/Sidebar/IconRail.tsx',
  'components/Sidebar/ProjectAvatar.tsx',
  'components/Sidebar/Sidebar.tsx',
  'components/Sidebar/WorktreeDialog.tsx',
  'components/TopBar/TopBar.tsx',
  'components/WhatsNewModal/WhatsNewModal.tsx',
  'components/Wiki/wiki-api.ts',
  'components/WorkspaceAssistant/AssistantBar.tsx',
  'components/WorkspacePanel/ConnectedAgentsSection.tsx',
  'components/WorkspacePanel/UrlsPortsSection.tsx',
  'components/WorkspacePanel/WorkspaceApiSection.tsx',
  'components/WorkspacePanel/WorkspaceCompletionSoundToggle.tsx',
  'components/common/HeartbeatSessionPicker.tsx',
  'hooks/useCursorMigrationCheck.ts',
  'hooks/useGit.ts',
  'hooks/useTunnelUrls.ts',
  'lib/context-stack.ts',
  'lib/daemon-settings.ts',
  'lib/external-drop-router.ts',
  'lib/federation.ts',
  'lib/heartbeat-delivery.ts',
  'lib/llmDaemonClient.ts',
  'lib/pick-remote-image.ts',
  'lib/start-clone-to.ts',
  // Home M3 / Q4: the assistant acts only on rooms on the window's server.
  'lib/workspace-ops-router.ts',
  'stores/active-agents.ts',
  'stores/assistant.ts',
  'stores/claude-auth.ts',
  'stores/custom-themes.ts',
  'stores/feedback.ts',
  'stores/focus-groups.ts',
  'stores/heartbeat-sessions.ts',
  'stores/presence.ts',
  'stores/presets.ts',
  'stores/project-groups.ts',
  'stores/projects.ts',
  // Home M3: the primary room is built on the primary scope.
  'stores/room.ts',
  'stores/subscription-usage.ts',
  'stores/tabs.ts',
  'stores/timer.ts',
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
      'components/SessionView/useChatTranscript.ts',
    ]) {
      expect([f, /\bprimaryScope\(\)/.test(read(f))]).toEqual([f, false])
    }
  })
})
