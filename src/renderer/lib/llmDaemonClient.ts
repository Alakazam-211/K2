// Phase 2 Unit 2 — daemon-side LLM client.
//
// Replaces every `invoke('assistant_*')` Tauri command with a direct
// fetch against the k2so-daemon's `/cli/llm/*` endpoints. The daemon
// owns the LLM worker subprocess + supervisor (timeout / RSS watchdog
// / max-concurrency gate / crash isolation) so the renderer talks to
// it the same way K2 Connect / Mobile Companion will:
//
//   await llmStatus()          → { loaded, modelPath, downloading, ... }
//   await llmCheck()           → { ok: true } | { ok: false, reason }
//   await llmLoadModel(path)   → { path: '<final-path>' }
//   await llmDownloadDefault() → { started: true }
//   await llmChat({...})       → { raw, parsed, debugPasses }
//
// Mirrors the daemon-client helper pattern Unit 1 established in
// `components/Settings/sections/CompanionSection.tsx`. Errors are
// surfaced as plain `Error` instances so callers can `try/catch`
// without parsing HTTP status codes themselves.

import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'

export interface LlmStatus {
  loaded: boolean
  modelPath: string | null
  downloading: boolean
  inflight: number
  queued: number
}

export interface LlmCheck {
  ok: boolean
  reason?: string
}

// Matches `crates/k2so-daemon/src/llm_routes.rs::ChatResponse`.
export interface ChatResponse {
  raw: string
  parsed: {
    toolCalls?: Array<{ tool: string; args: Record<string, unknown> }>
    tool_calls?: Array<{ tool: string; args: Record<string, unknown> }>
    message?: string
  }
  debugPasses: Array<{ prompt: string; rawOutput: string }>
}

async function daemonGet(pathSuffix: string): Promise<Response> {
  const creds = await getDaemonWs()
  return fetch(
    `${daemonHttpBase(creds)}/cli/llm/${pathSuffix}?token=${creds.token}`,
    { method: 'GET' },
  )
}

async function daemonPostJson(
  pathSuffix: string,
  body: unknown,
): Promise<Response> {
  const creds = await getDaemonWs()
  return fetch(
    `${daemonHttpBase(creds)}/cli/llm/${pathSuffix}?token=${creds.token}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    },
  )
}

async function readJsonOrThrow<T>(res: Response, label: string): Promise<T> {
  const text = await res.text()
  if (!res.ok) {
    let msg = text || `${label} ${res.status}`
    try {
      const parsed = JSON.parse(text)
      if (parsed && typeof parsed.error === 'string') msg = parsed.error
    } catch {
      /* keep raw text */
    }
    throw new Error(msg)
  }
  try {
    return JSON.parse(text) as T
  } catch (e) {
    throw new Error(`${label}: invalid JSON response (${String(e)})`)
  }
}

export async function llmStatus(): Promise<LlmStatus> {
  const res = await daemonGet('status')
  return readJsonOrThrow<LlmStatus>(res, 'llm status')
}

export async function llmCheck(): Promise<LlmCheck> {
  const res = await daemonGet('check')
  return readJsonOrThrow<LlmCheck>(res, 'llm check')
}

export async function llmLoadModel(path: string): Promise<string> {
  const res = await daemonPostJson('load-model', { path })
  const parsed = await readJsonOrThrow<{ path: string }>(res, 'llm load-model')
  return parsed.path
}

export async function llmDownloadDefault(): Promise<{ started: boolean; reason?: string }> {
  const res = await daemonPostJson('download-default', {})
  return readJsonOrThrow<{ started: boolean; reason?: string }>(res, 'llm download-default')
}

export interface ChatRequest {
  message: string
  workspacePath?: string
  isGitRepo?: boolean
}

export async function llmChat(req: ChatRequest): Promise<ChatResponse> {
  const res = await daemonPostJson('chat', req)
  return readJsonOrThrow<ChatResponse>(res, 'llm chat')
}
