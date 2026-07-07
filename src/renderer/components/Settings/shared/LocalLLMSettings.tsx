import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { useAssistantStore } from '@/stores/assistant'
import { useSettingsStore } from '@/stores/settings'
// Phase 2 Unit 2 — Tauri `assistant_*` commands deleted. Daemon owns
// /cli/llm/* and the renderer calls it directly via the daemon client.
import { llmCheck, llmDownloadDefault, llmLoadModel, llmStatus } from '@/lib/llmDaemonClient'

export function LocalLLMSettings(): React.JSX.Element {
  const { isDownloading, downloadProgress, modelLoaded } = useAssistantStore()
  const aiAssistantEnabled = useSettingsStore((s) => s.aiAssistantEnabled)
  const setAiAssistantEnabled = useSettingsStore((s) => s.setAiAssistantEnabled)
  // GH#8 — "Use local LLM to detect HITL" opt-in, gated on the model being
  // loaded and ready. The toggle is disabled (greyed, not clickable) until a
  // model is actually loaded; with no model there's nothing to run, so we
  // don't let the flag flip and we show a "Load a model to enable." hint.
  const useLlmHitlDetection = useSettingsStore((s) => s.useLlmHitlDetection)
  const setUseLlmHitlDetection = useSettingsStore((s) => s.setUseLlmHitlDetection)
  const modelReady = modelLoaded && !isDownloading
  const [modelPath, setModelPath] = useState<string | null>(null)
  const [modelExists, setModelExists] = useState<boolean | null>(null)
  const [customPath, setCustomPath] = useState('')
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loadingModel, setLoadingModel] = useState(false)

  useEffect(() => {
    llmStatus()
      .then((status) => {
        setModelPath(status.modelPath)
        if (status.modelPath) setCustomPath(status.modelPath)
      })
      .catch((e) => console.warn('[settings]', e))

    llmCheck()
      .then((res) => setModelExists(!!res.ok))
      .catch((e) => console.warn('[settings]', e))
  }, [modelLoaded])

  const handleDownload = useCallback(async () => {
    try {
      setLoadError(null)
      await llmDownloadDefault()
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  const handleLoadCustom = useCallback(async () => {
    if (!customPath.trim()) return
    setLoadingModel(true)
    setLoadError(null)
    try {
      const finalPath = await llmLoadModel(customPath.trim())
      setModelPath(finalPath)
      setCustomPath(finalPath)
      useAssistantStore.getState().setModelLoaded(true)
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoadingModel(false)
    }
  }, [customPath])

  return (
    <div>
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">AI Workspace Assistant</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        A local LLM that translates natural language into workspace operations. Press <kbd className="px-1 py-0.5 bg-white/[0.06] text-[var(--color-text-secondary)] font-mono text-[10px]">&#8984;L</kbd> to open.
        Runs entirely on your machine — no data is sent to external servers.
      </p>
      <div className="border border-[var(--color-border)]">
        {/* Enabled */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-primary)]">Enabled</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Disabling saves battery by not loading the model into memory</p>
          </div>
          <button
            onClick={() => setAiAssistantEnabled(!aiAssistantEnabled)}
            className="no-drag cursor-pointer flex-shrink-0 relative"
            style={{
              width: 36,
              height: 20,
              backgroundColor: aiAssistantEnabled ? 'var(--color-accent)' : 'var(--color-control-track-off)',
              border: 'none',
              transition: 'background-color 150ms'
            }}
          >
            <span
              style={{
                position: 'absolute',
                top: 2,
                left: aiAssistantEnabled ? 18 : 2,
                width: 16,
                height: 16,
                backgroundColor: 'var(--color-on-accent)',
                transition: 'left 150ms'
              }}
            />
          </button>
        </div>
        {/* Model Status */}
        <div className="px-4 py-3 border-b border-[var(--color-border)]">
          <span className="text-xs text-[var(--color-text-primary)]">Model Status</span>
          <div className="flex items-center gap-2 mt-2">
            <span
              className="w-2 h-2 flex-shrink-0"
              style={{ backgroundColor: modelLoaded ? 'var(--color-status-ok-soft)' : 'var(--color-status-error)' }}
            />
            <span className="text-xs text-[var(--color-text-secondary)]">
              {modelLoaded ? 'Model loaded and ready' : 'No model loaded'}
            </span>
          </div>
          {modelPath && (
            <p className="text-[10px] font-mono text-[var(--color-text-muted)] break-all mt-1">
              {modelPath}
            </p>
          )}
        </div>
        {/* GH#8 — "Use local LLM to detect HITL" opt-in. Gates whether
            `k2 talk`'s HITL detection runs the bundled 1.5B model (catches
            unmarked prompts) vs. regex-only. Usable ONLY when the model is
            loaded and ready — disabled (and the stored value left untouched)
            otherwise. The daemon reads this same flag server-side. */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="flex-1 min-w-0 mr-3">
            <span className={`text-xs ${modelReady ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-muted)]'}`}>
              Use local LLM to detect HITL states (off = regex only)
            </span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
              {!modelReady
                ? 'Load a model to enable.'
                : useLlmHitlDetection
                  ? 'On: k2 talk runs the bundled 1.5B model to spot human-in-the-loop prompts the regex misses (unmarked menus/confirmations). Costs some battery and CPU per check.'
                  : 'Off (default): k2 talk detects human-in-the-loop prompts with fast regex only — no model inference. Obvious prompts are still caught; unmarked ones may not be.'}
            </p>
          </div>
          <button
            onClick={() => { if (modelReady) void setUseLlmHitlDetection(!useLlmHitlDetection) }}
            disabled={!modelReady}
            className="no-drag flex-shrink-0 relative cursor-pointer disabled:cursor-default disabled:opacity-40"
            data-settings-id="general.use-llm-hitl-detection"
            style={{
              width: 36,
              height: 20,
              backgroundColor: useLlmHitlDetection && modelReady ? 'var(--color-accent)' : 'var(--color-control-track-off)',
              border: 'none',
              transition: 'background-color 150ms',
            }}
          >
            <span
              style={{
                position: 'absolute',
                top: 2,
                left: useLlmHitlDetection && modelReady ? 18 : 2,
                width: 16,
                height: 16,
                backgroundColor: 'var(--color-on-accent)',
                transition: 'left 150ms',
              }}
            />
          </button>
        </div>
        {/* Default Model */}
        <div className="px-4 py-3 border-b border-[var(--color-border)]">
            <span className="text-xs text-[var(--color-text-primary)]">Default Model</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-1 mb-2">
              Qwen2.5-1.5B-Instruct (Q4_K_M) — ~1.1GB download. Runs locally with Metal GPU acceleration.
            </p>
            {isDownloading ? (
              <div>
                <div className="flex items-center justify-between mb-1">
                  <span className="text-xs text-[var(--color-text-secondary)]">Downloading...</span>
                  <span className="text-xs font-mono text-[var(--color-text-muted)]">{Math.round(downloadProgress)}%</span>
                </div>
                <div className="h-1.5 bg-[var(--color-bg)] overflow-hidden">
                  <div
                    className="h-full bg-[var(--color-accent)] transition-all duration-300"
                    style={{ width: `${downloadProgress}%` }}
                  />
                </div>
              </div>
            ) : (
              <button
                onClick={handleDownload}
                disabled={modelExists === true && modelLoaded}
                className="px-3 py-1.5 text-xs bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] hover:bg-white/[0.08] transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-default no-drag"
              >
                {modelExists ? (modelLoaded ? 'Downloaded & Loaded' : 'Download & Load') : 'Download Default Model'}
              </button>
            )}
          </div>
          {/* Custom Model */}
          <div className="px-4 py-3">
            <span className="text-xs text-[var(--color-text-primary)]">Custom Model</span>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-1 mb-2">
              Point to any GGUF model file. It will be copied to <span className="font-mono">~/.k2/models/</span> automatically.
            </p>
            <div className="flex gap-2">
              <input
                type="text"
                value={customPath}
                onChange={(e) => setCustomPath(e.target.value)}
                placeholder="~/.k2/models/your-model.gguf"
                className="flex-1 px-2 py-1.5 text-xs font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)] no-drag"
              />
              <button
                onClick={handleLoadCustom}
                disabled={!customPath.trim() || loadingModel}
                className="px-3 py-1.5 text-xs bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] border border-[var(--color-border)] hover:bg-white/[0.08] transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-default no-drag flex-shrink-0"
              >
                {loadingModel ? 'Loading...' : 'Load'}
              </button>
            </div>
          </div>
        </div>
      {/* Error Display */}
      {loadError && (
        <div className="p-2 text-xs text-[var(--color-status-error-soft)] bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_20%,transparent)] mt-3">
          {loadError}
        </div>
      )}
    </div>
  )
}
