//! Phase 2 Unit 2 — daemon-side `/cli/llm/*` route handlers.
//!
//! Companion of `llm_host` (the subprocess supervisor). This module
//! provides the HTTP request/response glue:
//!
//! - `GET  /cli/llm/check`            — fast probe
//! - `GET  /cli/llm/status`           — model loaded? memory used? pids?
//! - `POST /cli/llm/chat`             — body: `{"message","workspacePath?","isGitRepo?"}`,
//!                                     returns `{"raw","parsed","debugPasses"}`
//! - `POST /cli/llm/load-model`       — body: `{"path"}`, copies into
//!                                     `~/.k2so/models/` if outside.
//! - `POST /cli/llm/download-default` — kicks off the default-model
//!                                     download in a background thread.
//!
//! The chat handler mirrors `src-tauri/src/commands/assistant.rs::
//! assistant_chat` byte-for-byte, including the two-pass file-tool
//! dance (LLM emits `list_files`/`search_files`, the daemon executes
//! them, feeds results back for a second inference). The two passes
//! share one in-flight slot — entire request is "one inference unit"
//! from the supervisor's gate perspective.
//!
//! # Stream vs blocking
//!
//! The PRD called for streaming token output; today's renderer is
//! fully blocking (it `invoke()`s assistant_chat and waits for the
//! full response, then renders). Migrating to streaming requires a
//! token-by-token worker protocol change AND a renderer rewrite. To
//! keep Unit 2 focused on the architectural keystone (supervisor +
//! crash isolation) we mirror today's blocking shape. A future unit
//! can add a `/cli/llm/chat-stream` WS route once we have a worker
//! protocol that emits incremental tokens.

use std::sync::Arc;

use k2_core::log_debug;
use serde::{Deserialize, Serialize};

use crate::cli_response::CliResponse;
use crate::llm_host::{self, GenerateError, LlmHost};
use crate::session_events::{self, SessionEvent};

/// 0.39.39 (#675.1) — push the current LLM host status onto the
/// `/cli/sessions/events` broadcast so the renderer can drop its
/// `/cli/llm/status` poll (assistant.ts). Best-effort; the broadcast
/// `let _ =`-swallows the no-subscribers case. `download_percent` is
/// carried only on progress emits (None otherwise).
pub(crate) fn emit_llm_status(download_percent: Option<f64>) {
    let status = llm_host::shared().status();
    let _ = session_events::emit(SessionEvent::LlmStatusChanged {
        loaded: status.loaded,
        model_path: status.model_path,
        downloading: status.downloading,
        download_percent,
    });
}

// ─── Request / response shapes ───────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatRequest {
    message: String,
    #[serde(default)]
    workspace_path: Option<String>,
    #[serde(default)]
    is_git_repo: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DebugPass {
    prompt: String,
    raw_output: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatResponse {
    raw: String,
    parsed: k2_core::llm::tools::AssistantResponse,
    debug_passes: Vec<DebugPass>,
}

#[derive(Debug, Deserialize)]
struct LoadModelRequest {
    path: String,
}

// ─── Handlers ────────────────────────────────────────────────────────

/// `GET /cli/llm/check` — lightweight probe. Returns `{"ok":true}` if
/// a model file is currently configured AND exists on disk, otherwise
/// `{"ok":false,"reason":"..."}`. The renderer calls this before
/// trying to chat; it never touches the supervisor's subprocess
/// machinery.
pub fn handle_check() -> CliResponse {
    let host = llm_host::shared();
    match host.model_path() {
        Some(p) if std::path::Path::new(&p).exists() => {
            CliResponse::ok_json(r#"{"ok":true}"#.to_string())
        }
        Some(p) => CliResponse::ok_json(format!(
            r#"{{"ok":false,"reason":"model path {} no longer exists"}}"#,
            serde_json::to_string(&p).unwrap_or_else(|_| "\"?\"".into())
        )),
        None => CliResponse::ok_json(
            r#"{"ok":false,"reason":"no model configured"}"#.to_string(),
        ),
    }
}

/// `GET /cli/llm/status` — full status snapshot.
pub fn handle_status() -> CliResponse {
    let host = llm_host::shared();
    let status = host.status();
    CliResponse::ok_json(serde_json::to_string(&status).unwrap_or_else(|_| "{}".into()))
}

/// `POST /cli/llm/load-model` — body: `{"path": "..."}`. Copies the
/// model into `~/.k2so/models/` if it's outside that dir, then sets
/// the host's `model_path`. Returns `{"path":"<final-path>"}`.
pub fn handle_load_model(body: &[u8]) -> CliResponse {
    let parsed: LoadModelRequest = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.path.is_empty() {
        return CliResponse::bad_request("path must not be empty");
    }
    let src = std::path::PathBuf::from(&parsed.path);
    if !src.exists() {
        return CliResponse::bad_request(format!("file not found: {}", parsed.path));
    }

    let models_dir = match k2_core::llm::download::models_dir() {
        Ok(d) => d,
        Err(e) => return CliResponse::bad_request(format!("models_dir: {e}")),
    };

    let final_path = if src.starts_with(&models_dir) {
        parsed.path.clone()
    } else {
        if let Err(e) = std::fs::create_dir_all(&models_dir) {
            return CliResponse::bad_request(format!("create models dir: {e}"));
        }
        let filename = match src.file_name() {
            Some(f) => f,
            None => return CliResponse::bad_request("invalid file path"),
        };
        let dest = models_dir.join(filename);
        log_debug!("[llm-host] copying model to {:?}", dest);
        if let Err(e) = std::fs::copy(&src, &dest) {
            return CliResponse::bad_request(format!("copy model: {e}"));
        }
        dest.to_string_lossy().to_string()
    };

    let host = llm_host::shared();
    host.set_model_path(final_path.clone());
    // #675.1 — model is now loaded/ready; push the new status.
    emit_llm_status(None);
    CliResponse::ok_json(format!(
        r#"{{"path":{}}}"#,
        serde_json::to_string(&final_path).unwrap_or_else(|_| "\"\"".into())
    ))
}

/// `POST /cli/llm/download-default` — fire-and-forget download of the
/// default Qwen2.5-1.5B GGUF. Returns immediately with
/// `{"started":true}` or `{"started":false,"reason":"already downloading"}`.
/// Progress is broadcast over the daemon's event channel as
/// `assistant:download-progress` so /events WS subscribers see it.
pub fn handle_download_default(event_tx: &Arc<tokio::sync::broadcast::Sender<crate::events::WireEvent>>) -> CliResponse {
    let host = llm_host::shared();
    if !host.try_begin_download() {
        return CliResponse::ok_json(
            r#"{"started":false,"reason":"already downloading"}"#.to_string(),
        );
    }

    let dest = match k2_core::llm::download::default_model_path() {
        Ok(p) => p,
        Err(e) => {
            host.end_download();
            return CliResponse::bad_request(format!("default_model_path: {e}"));
        }
    };
    let dest_str = dest.to_string_lossy().to_string();
    let url = k2_core::llm::download::DEFAULT_MODEL_URL.to_string();
    let host_for_thread = host.clone();
    let event_tx_for_thread = event_tx.clone();

    // #675.1 — download just began (downloading flag is now set).
    emit_llm_status(Some(0.0));

    std::thread::spawn(move || {
        let event_tx_progress = event_tx_for_thread.clone();
        let result = k2_core::llm::download::download_model(
            &url,
            &dest_str,
            move |p| {
                // Emit progress over the daemon's legacy event channel
                // (kept for back-compat) …
                let _ = event_tx_progress.send(crate::events::WireEvent {
                    event: "assistant:download-progress".to_string(),
                    payload: serde_json::json!({
                        "percent": p.percent,
                        "bytesDownloaded": p.bytes_downloaded,
                        "totalBytes": p.total_bytes,
                    }),
                });
                // … and #675.1 over the canonical session-events spine so
                // the renderer can consolidate onto one subscription.
                emit_llm_status(Some(p.percent));
            },
        );

        host_for_thread.end_download();

        match result {
            Ok(()) => {
                host_for_thread.set_model_path(dest_str.clone());
                log_debug!("[llm-host] default model downloaded to {dest_str}");
            }
            Err(e) => {
                log_debug!("[llm-host] default model download failed: {e}");
            }
        }
        // #675.1 — download finished (success or failure): push the
        // final status (downloading flag now cleared).
        emit_llm_status(None);
    });

    CliResponse::ok_json(r#"{"started":true}"#.to_string())
}

/// `POST /cli/llm/chat` — full chat handler. Mirrors the legacy
/// Tauri `assistant_chat` exactly: builds a system prompt with the
/// optional `is_git_repo` flag, runs one inference pass, if the LLM
/// emits `list_files`/`search_files` tool calls executes them against
/// the supplied `workspace_path` and runs a second pass with the
/// listing baked into the prompt.
///
/// Errors map to HTTP statuses:
/// - 429 — admission queue full (`GenerateError::TooManyRequests`)
/// - 503 — no model loaded (`GenerateError::NoModel`)
/// - 504 — worker timeout
/// - 502 — worker crash (signal, abort, OOM, non-zero exit)
/// - 500 — internal supervisor error
pub fn handle_chat(body: &[u8]) -> CliResponse {
    let parsed: ChatRequest = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.message.is_empty() {
        return CliResponse::bad_request("message must not be empty");
    }

    let host = llm_host::shared();
    let system_prompt =
        k2_core::llm::tools::build_system_prompt(parsed.is_git_repo.unwrap_or(false));

    log_debug!("[llm-host] user message: {}", parsed.message);
    log_debug!(
        "[llm-host] system prompt length: {} chars",
        system_prompt.len()
    );

    let timeout = llm_host::default_timeout();
    let mut debug_passes: Vec<DebugPass> = Vec::new();

    // Pass 1
    let raw1 = match host.generate(&system_prompt, &parsed.message, timeout) {
        Ok(s) => s,
        Err(e) => return generate_error_response(&e),
    };
    log_debug!("[llm-host] raw LLM response (pass 1): {raw1}");
    debug_passes.push(DebugPass {
        prompt: parsed.message.clone(),
        raw_output: raw1.clone(),
    });
    let parsed_resp = k2_core::llm::tools::parse_llm_response(&raw1);

    // Possible pass 2 — if the LLM asked for file listings, execute
    // them and run another inference.
    if let k2_core::llm::tools::AssistantResponse::ToolCalls { ref tool_calls } = parsed_resp {
        let has_file_tools = tool_calls
            .iter()
            .any(|c| c.tool == "list_files" || c.tool == "search_files");

        if has_file_tools {
            if let Some(ws_path) = parsed.workspace_path.as_ref() {
                if let Some(listing_text) = execute_file_tools(tool_calls, ws_path) {
                    let action_calls: Vec<_> = tool_calls
                        .iter()
                        .filter(|c| c.tool != "list_files" && c.tool != "search_files")
                        .cloned()
                        .collect();
                    let follow_up = format!(
                        "File listing results:\n{listing_text}\n\nOriginal request: {}\n\nNow output the tool_calls to fulfill the request using the file paths above.",
                        parsed.message
                    );
                    log_debug!("[llm-host] follow-up prompt (pass 2): {follow_up}");

                    let raw2 = match host.generate(&system_prompt, &follow_up, timeout) {
                        Ok(s) => s,
                        Err(e) => return generate_error_response(&e),
                    };
                    log_debug!("[llm-host] raw LLM response (pass 2): {raw2}");
                    debug_passes.push(DebugPass {
                        prompt: follow_up,
                        raw_output: raw2.clone(),
                    });
                    let parsed2 = k2_core::llm::tools::parse_llm_response(&raw2);

                    let final_parsed = if action_calls.is_empty() {
                        parsed2
                    } else if let k2_core::llm::tools::AssistantResponse::ToolCalls {
                        tool_calls: mut pass2_calls,
                    } = parsed2
                    {
                        let mut merged = action_calls;
                        merged.append(&mut pass2_calls);
                        k2_core::llm::tools::AssistantResponse::ToolCalls {
                            tool_calls: merged,
                        }
                    } else if action_calls.is_empty() {
                        parsed2
                    } else {
                        k2_core::llm::tools::AssistantResponse::ToolCalls {
                            tool_calls: action_calls,
                        }
                    };

                    let resp = ChatResponse {
                        raw: raw2,
                        parsed: final_parsed,
                        debug_passes,
                    };
                    return CliResponse::ok_json(
                        serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into()),
                    );
                }
            }
        }
    }

    let resp = ChatResponse {
        raw: raw1,
        parsed: parsed_resp,
        debug_passes,
    };
    CliResponse::ok_json(serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into()))
}

/// Execute any `list_files` / `search_files` tool calls server-side
/// and return the combined listing. Mirrors the helper of the same
/// name in `src-tauri/src/commands/assistant.rs`.
fn execute_file_tools(
    tool_calls: &[k2_core::llm::tools::ToolCall],
    workspace_path: &str,
) -> Option<String> {
    let mut results = Vec::new();
    for call in tool_calls {
        match call.tool.as_str() {
            "list_files" => {
                let rel_path = call
                    .args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".");
                let abs_path = if rel_path == "." || rel_path.is_empty() {
                    std::path::PathBuf::from(workspace_path)
                } else {
                    std::path::Path::new(workspace_path).join(rel_path)
                };
                let listing = k2_core::llm::file_index::list_directory(
                    abs_path.to_string_lossy().as_ref(),
                );
                results.push(format!("[list_files: {rel_path}]\n{listing}"));
            }
            "search_files" => {
                let query = call
                    .args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !query.is_empty() {
                    let search_results =
                        k2_core::llm::file_index::search_files(workspace_path, query);
                    results.push(format!("[search_files: \"{query}\"]\n{search_results}"));
                }
            }
            _ => {}
        }
    }
    if results.is_empty() {
        None
    } else {
        Some(results.join("\n"))
    }
}

/// Translate a `GenerateError` into the appropriate HTTP response.
fn generate_error_response(e: &GenerateError) -> CliResponse {
    let body = serde_json::json!({ "error": e.message() }).to_string();
    let status: &'static str = match e {
        GenerateError::TooManyRequests(_) => "429 Too Many Requests",
        GenerateError::NoModel(_) => "503 Service Unavailable",
        GenerateError::Timeout(_) => "504 Gateway Timeout",
        GenerateError::WorkerCrashed(_) => "502 Bad Gateway",
        GenerateError::Internal(_) => "500 Internal Server Error",
    };
    CliResponse {
        status,
        content_type: "application/json",
        body,
    }
}

// Silence "unused import for Arc/LlmHost" when this module compiles
// in isolation (Arc<LlmHost> is the public type that
// download-default takes via the host singleton). Keep them around
// in case we add tests that inject a host.
#[allow(dead_code)]
fn _type_anchors(_: Arc<LlmHost>) {}
