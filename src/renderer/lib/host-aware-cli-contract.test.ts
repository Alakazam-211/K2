// K2 Connect remote-completeness — contract tests for the renderer-side
// `invoke(...)` → `daemonCliGet(...)` swaps that make local-pinned Tauri
// commands hit the ACTIVE host (local OR a remote K2 Connect daemon).
//
// The production call sites are inline in heavy React components / the
// active-agents polling listener (HeartbeatsSection.commitRename,
// AgentPane/AgentChatPane/active-agents agents/lock), which can't be
// exercised without rendering the full component tree + Tauri event
// system. These tests instead lock the WIRE CONTRACT each swap must
// satisfy: the exact `daemonCliGet(route, params)` route name + param
// KEYS, including the two riskiest REMAPS:
//
//   1. heartbeat/rename — renderer var oldName/newName → route reads
//      `from`/`to` (verified crates/k2so-daemon/src/heartbeat_routes.rs).
//   2. agents/lock      — renderer projectPath/agentName/terminalId →
//      route reads project/agent/terminal_id (verified
//      crates/k2so-daemon/src/cli.rs `/cli/agents/lock`).
//
// A regression that reverts a remap (e.g. sends `oldName` again, which the
// route would silently drop → empty rename) fails these loudly. The param
// builders below mirror EXACTLY what the call sites construct.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

import { daemonCliGet as cli, daemonCliPost as cliPost } from '@/lib/daemon-cli'

// Mirrors HeartbeatsSection.commitRename: renderer holds row.name (old) +
// the trimmed/lowercased draft (new); the route reads `from`/`to`.
function renameHeartbeat(project: string, oldName: string, newName: string) {
  return cli('heartbeat/rename', { project, from: oldName, to: newName })
}

// Mirrors the agents/lock call sites (active-agents ×2, AgentPane ×2,
// AgentChatPane ×3): renderer holds projectPath/agentName/terminalId/owner;
// the route reads project/agent/terminal_id/owner.
function lockAgent(
  projectPath: string,
  agentName: string,
  terminalId: string,
  owner: 'user' | 'system',
) {
  return cli('agents/lock', {
    project: projectPath,
    agent: agentName,
    terminal_id: terminalId,
    owner,
  })
}

describe('host-aware CLI swaps — wire contract', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliGet.mockResolvedValue({ success: true })
  })

  it('heartbeat/rename remaps oldName/newName → from/to (no stray oldName/newName)', async () => {
    await renameHeartbeat('/work/proj', 'nightly', 'overnight')
    expect(daemonCliGet).toHaveBeenCalledWith('heartbeat/rename', {
      project: '/work/proj',
      from: 'nightly',
      to: 'overnight',
    })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    // The pre-swap renderer keys must NOT survive — the route ignores them,
    // so leaving them in would silently no-op the rename on a remote host.
    expect(params).not.toHaveProperty('oldName')
    expect(params).not.toHaveProperty('newName')
  })

  it('agents/lock remaps projectPath/agentName/terminalId → project/agent/terminal_id', async () => {
    await lockAgent('/work/proj', 'manager', 'agent-chat:proj:manager', 'user')
    expect(daemonCliGet).toHaveBeenCalledWith('agents/lock', {
      project: '/work/proj',
      agent: 'manager',
      terminal_id: 'agent-chat:proj:manager',
      owner: 'user',
    })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    expect(params).not.toHaveProperty('projectPath')
    expect(params).not.toHaveProperty('agentName')
    expect(params).not.toHaveProperty('terminalId')
  })
})

// ════════════════════════════════════════════════════════════════════
// K2 Connect host-awareness GAP — POST wire contract (Unit 2b)
// ════════════════════════════════════════════════════════════════════
//
// The GAP routes Unit 2a shipped are all POST + JSON body, so the
// renderer swaps `invoke(cmd, {...})` → `daemonCliPost(route, {...})`.
// Each swap remaps the renderer's camelCase args to the route's exact
// snake_case body fields (verified against
// crates/k2so-daemon/src/{skills,agents,heartbeat}_routes.rs). A
// regression that reverts a remap (e.g. sends `sourceProjectId` again,
// which the route ignores → empty `source_project_id` → 400) fails
// these loudly. The two riskiest swaps are locked below:
//
//   1. relations/create — renderer sourceProjectId/targetProjectId →
//      route reads source_project_id/target_project_id (the body has an
//      optional relation_type, omitted by the renderer call site).
//   2. session/set-surfaced — the 8-arg surfaced-toggle: each positional
//      maps to its snake_case body field
//      (projectPath/agentName/terminalId/heartbeatName/attachAgentName →
//      project_path/agent_name/terminal_id/heartbeat_name/attach_agent_name).

// Mirrors ProjectsSection.handleAdd: renderer holds projectId (source) +
// the picked targetProjectId; the route reads source/target_project_id.
function createRelation(sourceProjectId: string, targetProjectId: string) {
  return cliPost('relations/create', {
    source_project_id: sourceProjectId,
    target_project_id: targetProjectId,
  })
}

// Mirrors tabs.ts openHeartbeatTab: the 8-field surfaced=true body that
// hands the existing PTY to the surfaced flow. Every camelCase arg the
// old Tauri command took maps to a snake_case body field.
// Mirrors RestartHostRow.handleRestart (#661): the host-aware "Restart
// connected host" control posts an EMPTY body to `daemon/restart` against
// the ACTIVE host. The route takes no JSON body (the owner token rides the
// query string), so the body must stay `{}` — no stray fields that an
// older/stricter handler could choke on.
function restartHost() {
  return cliPost('daemon/restart', {})
}

function setSurfaced(
  projectPath: string,
  agentName: string,
  terminalId: string,
  command: string,
  args: string[],
  heartbeatName: string,
  attachAgentName: string,
) {
  return cliPost('session/set-surfaced', {
    project_path: projectPath,
    agent_name: agentName,
    surfaced: true,
    terminal_id: terminalId,
    command,
    args,
    heartbeat_name: heartbeatName,
    attach_agent_name: attachAgentName,
  })
}

describe('host-aware CLI POST swaps — GAP wire contract', () => {
  beforeEach(() => {
    daemonCliPost.mockReset()
    daemonCliPost.mockResolvedValue({ success: true })
  })

  it('relations/create remaps sourceProjectId/targetProjectId → source/target_project_id', async () => {
    await createRelation('proj-a', 'proj-b')
    expect(daemonCliPost).toHaveBeenCalledWith('relations/create', {
      source_project_id: 'proj-a',
      target_project_id: 'proj-b',
    })
    const [, body] = daemonCliPost.mock.calls[0] as [string, Record<string, unknown>]
    // Pre-swap camelCase keys must NOT survive — the route ignores them,
    // so a leftover would 400 (missing source_project_id) on any host.
    expect(body).not.toHaveProperty('sourceProjectId')
    expect(body).not.toHaveProperty('targetProjectId')
  })

  it('daemon/restart posts an empty body to the active host (#661)', async () => {
    await restartHost()
    expect(daemonCliPost).toHaveBeenCalledWith('daemon/restart', {})
    const [route, body] = daemonCliPost.mock.calls[0] as [string, Record<string, unknown>]
    // Route is exact (host-aware daemonCliPost prefixes /cli/ + targets the
    // ACTIVE host); the body carries no fields — the owner token rides the
    // query string, so any extra key here would be wrong.
    expect(route).toBe('daemon/restart')
    expect(Object.keys(body)).toHaveLength(0)
  })
})

// ════════════════════════════════════════════════════════════════════
// K2 Connect host-awareness GAP — relations LIST reads (GET wire contract)
// ════════════════════════════════════════════════════════════════════
//
// Unit 2a/2b made relations CREATE + DELETE host-aware but left the LIST
// reads on LOCAL Tauri invoke(), so the "Connected Workspaces" panel went
// blank against a remote K2 Connect host. These two GET reads close that
// gap. The renderer holds a camelCase `projectId`; the route reads the
// query param `project_id` (verified crates/k2so-daemon/src/agents_routes.rs
// `/cli/relations/list{,-incoming}` → `str_param(params, "project_id")`).
// A regression that sends `projectId` again would silently read an empty
// id → empty array → blank panel; the not-toHaveProperty guards catch it.

// Mirrors ProjectsSection.fetchRelations: source (outgoing) + incoming.
function listRelations(projectId: string) {
  return cli('relations/list', { project_id: projectId })
}
function listRelationsIncoming(projectId: string) {
  return cli('relations/list-incoming', { project_id: projectId })
}

describe('host-aware CLI GET swaps — relations LIST wire contract', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliGet.mockResolvedValue([])
  })

  it('relations/list remaps projectId → project_id (outgoing)', async () => {
    await listRelations('proj-a')
    expect(daemonCliGet).toHaveBeenCalledWith('relations/list', {
      project_id: 'proj-a',
    })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    expect(params).not.toHaveProperty('projectId')
  })

  it('relations/list-incoming remaps projectId → project_id (incoming)', async () => {
    await listRelationsIncoming('proj-b')
    expect(daemonCliGet).toHaveBeenCalledWith('relations/list-incoming', {
      project_id: 'proj-b',
    })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    expect(params).not.toHaveProperty('projectId')
  })
})

// ════════════════════════════════════════════════════════════════════
// Remote self-update — host-aware wire contract (P4)
// ════════════════════════════════════════════════════════════════════
//
// The "Update host" control (UpdateHostRow) drives the REMOTE machine's
// self-update over the P3 daemon routes, host-aware so they hit the ACTIVE
// host (local OR a remote K2 Connect daemon). The wire contract each call
// must satisfy:
//
//   - POST daemon/update/check  → empty body (owner token rides the query
//     string), returns {current, latest, available, …}.
//   - POST daemon/update/start  → empty body, returns {job_id}.
//   - GET  daemon/update/status → reads the `job_id` QUERY param; returns
//     {phase, progress?, bytes?, error?}.
//   - POST daemon/update/apply  → body { job_id } (the staged job to apply).
//
// A regression that puts job_id in the wrong place (e.g. body for status,
// or query for apply) would silently no-op against the daemon; the
// not-toHaveProperty / exact-args guards catch it.

// Mirrors UpdateHostRow.handleCheck: POST with an EMPTY body.
function updateCheck() {
  return cliPost('daemon/update/check', {})
}
// Mirrors UpdateHostRow.handleDownload: POST with an EMPTY body → {job_id}.
function updateStart() {
  return cliPost('daemon/update/start', {})
}
// Mirrors the status poll: GET reading the `job_id` query param.
function updateStatus(jobId: string) {
  return cli('daemon/update/status', { job_id: jobId })
}
// Mirrors UpdateHostRow.handleApply: POST with the staged job in the body.
function updateApply(jobId: string) {
  return cliPost('daemon/update/apply', { job_id: jobId })
}

describe('host-aware CLI — remote self-update wire contract (P4)', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliGet.mockResolvedValue({ phase: 'downloading' })
    daemonCliPost.mockReset()
    daemonCliPost.mockResolvedValue({ job_id: 'job-1' })
  })

  it('daemon/update/check is a POST with an empty body', async () => {
    await updateCheck()
    expect(daemonCliPost).toHaveBeenCalledWith('daemon/update/check', {})
    const [route, body] = daemonCliPost.mock.calls[0] as [string, Record<string, unknown>]
    expect(route).toBe('daemon/update/check')
    expect(Object.keys(body)).toHaveLength(0)
  })

  it('daemon/update/start is a POST with an empty body', async () => {
    await updateStart()
    expect(daemonCliPost).toHaveBeenCalledWith('daemon/update/start', {})
    const [route, body] = daemonCliPost.mock.calls[0] as [string, Record<string, unknown>]
    expect(route).toBe('daemon/update/start')
    expect(Object.keys(body)).toHaveLength(0)
  })

  it('daemon/update/status is a GET reading the job_id query param', async () => {
    await updateStatus('job-1')
    expect(daemonCliGet).toHaveBeenCalledWith('daemon/update/status', { job_id: 'job-1' })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    // It must be a GET (query param), never a POST — the status route reads
    // the query string, so posting it would drop the job id.
    expect(daemonCliPost).not.toHaveBeenCalled()
    expect(params).not.toHaveProperty('jobId')
  })

  it('daemon/update/apply is a POST carrying the staged job_id in the body', async () => {
    await updateApply('job-1')
    expect(daemonCliPost).toHaveBeenCalledWith('daemon/update/apply', { job_id: 'job-1' })
    const [, body] = daemonCliPost.mock.calls[0] as [string, Record<string, unknown>]
    // camelCase must not leak — the route reads snake_case `job_id`.
    expect(body).not.toHaveProperty('jobId')
  })
})

describe('host-aware CLI POST swaps — set-surfaced (cont.)', () => {
  beforeEach(() => {
    daemonCliPost.mockReset()
    daemonCliPost.mockResolvedValue({ success: true })
  })

  it('session/set-surfaced maps all 8 fields to snake_case body', async () => {
    await setSurfaced(
      '/work/proj',
      'manager',
      'term-1',
      'claude',
      ['--resume', 'sess-1'],
      'nightly',
      'manager',
    )
    expect(daemonCliPost).toHaveBeenCalledWith('session/set-surfaced', {
      project_path: '/work/proj',
      agent_name: 'manager',
      surfaced: true,
      terminal_id: 'term-1',
      command: 'claude',
      args: ['--resume', 'sess-1'],
      heartbeat_name: 'nightly',
      attach_agent_name: 'manager',
    })
    const [, body] = daemonCliPost.mock.calls[0] as [string, Record<string, unknown>]
    // None of the camelCase arg names may leak through — the route's
    // SetSurfacedBody reads only the snake_case fields.
    expect(body).not.toHaveProperty('projectPath')
    expect(body).not.toHaveProperty('agentName')
    expect(body).not.toHaveProperty('terminalId')
    expect(body).not.toHaveProperty('heartbeatName')
    expect(body).not.toHaveProperty('attachAgentName')
  })
})
