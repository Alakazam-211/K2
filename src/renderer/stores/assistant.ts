import { create } from 'zustand'
import { listen } from '@tauri-apps/api/event'
import { llmStatus } from '@/lib/llmDaemonClient'
import { serverSupports } from '@/lib/server-capabilities'
import { onLlmStatusChanged, onAppHello } from '@/stores/session-events'

// Phase 2 Unit 2 — `invoke('assistant_*')` calls retired. The daemon
// now owns the LLM lifecycle; the store polls `/cli/llm/status`
// directly (see `lib/llmDaemonClient.ts`). The download-progress
// event still flows via Tauri's event bus for now — daemon broadcasts
// `assistant:download-progress` on `/events` and Tauri's
// `daemon_events.rs` re-emits it onto the local event bus.

/** Maximum number of commands to keep in history. */
const MAX_HISTORY = 50
/** Maximum number of interaction log entries to keep. */
const MAX_LOG = 30

/** A single LLM inference pass (matches backend DebugPass). */
export interface DebugPass {
  prompt: string
  rawOutput: string
}

/** A full interaction log entry — one per user command. */
export interface InteractionLogEntry {
  /** When this command was sent. */
  timestamp: number
  /** The user's original message. */
  message: string
  /** The result summary shown to the user (or error). */
  result: string
  /** The parsed tool calls / message from the LLM. */
  parsed: unknown
  /** Raw LLM output from each inference pass. */
  debugPasses: DebugPass[]
}

interface AssistantState {
  isOpen: boolean
  isLoading: boolean
  isDownloading: boolean
  downloadProgress: number
  modelLoaded: boolean
  lastResult: string | null
  /** Command history, most recent last. */
  history: string[]
  /** Full interaction log for debugging, most recent last. */
  interactionLog: InteractionLogEntry[]
  /** Whether the debug log panel is visible. */
  showDebugLog: boolean

  open: () => void
  close: () => void
  toggle: () => void
  setLoading: (loading: boolean) => void
  setDownloading: (downloading: boolean, progress?: number) => void
  setModelLoaded: (loaded: boolean) => void
  setLastResult: (result: string | null) => void
  /** Add a command to history (deduplicates consecutive repeats). */
  addToHistory: (command: string) => void
  /** Log a full interaction for debugging. */
  logInteraction: (entry: InteractionLogEntry) => void
  /** Toggle the debug log panel. */
  toggleDebugLog: () => void
  /** Clear the interaction log. */
  clearLog: () => void
}

export const useAssistantStore = create<AssistantState>((set) => ({
  isOpen: false,
  isLoading: false,
  isDownloading: false,
  downloadProgress: 0,
  modelLoaded: false,
  lastResult: null,
  history: [],
  interactionLog: [{
    timestamp: Date.now(),
    message: 'system',
    result: 'Loading model...',
    parsed: null,
    debugPasses: [],
  }],
  showDebugLog: false,

  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false, lastResult: null }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
  setLoading: (loading) => set({ isLoading: loading }),
  setDownloading: (downloading, progress) =>
    set({
      isDownloading: downloading,
      downloadProgress: progress ?? (downloading ? 0 : 100)
    }),
  setModelLoaded: (loaded) => set((s) => {
    // Log model status to console so user sees it
    if (loaded && !s.modelLoaded) {
      const entry: InteractionLogEntry = {
        timestamp: Date.now(),
        message: 'system',
        result: 'Model loaded — ready for commands',
        parsed: null,
        debugPasses: [],
      }
      const log = [...s.interactionLog, entry]
      if (log.length > MAX_LOG) log.splice(0, log.length - MAX_LOG)
      return { modelLoaded: loaded, interactionLog: log }
    }
    return { modelLoaded: loaded }
  }),
  setLastResult: (result) => set({ lastResult: result }),
  addToHistory: (command) =>
    set((s) => {
      // Don't add consecutive duplicates
      if (s.history.length > 0 && s.history[s.history.length - 1] === command) {
        return s
      }
      const updated = [...s.history, command]
      // Trim to max size
      if (updated.length > MAX_HISTORY) {
        updated.splice(0, updated.length - MAX_HISTORY)
      }
      return { history: updated }
    }),
  logInteraction: (entry) =>
    set((s) => {
      const updated = [...s.interactionLog, entry]
      if (updated.length > MAX_LOG) {
        updated.splice(0, updated.length - MAX_LOG)
      }
      return { interactionLog: updated }
    }),
  toggleDebugLog: () => set((s) => ({ showDebugLog: !s.showDebugLog })),
  clearLog: () => set({ interactionLog: [] }),
}))

// Apply a single LLM status snapshot to the store. Shared by the one-shot
// snapshot fetch (push path), the WS re-snapshot on (re)connect, and the
// polling fallback (older / remote daemon without broadcasts).
const applyLlmStatus = (status: {
  loaded: boolean
  downloading: boolean
}): void => {
  const store = useAssistantStore.getState()
  if (status.loaded) {
    store.setModelLoaded(true)
  } else if (status.downloading) {
    store.setDownloading(true)
  }
}

// One-shot status snapshot (current truth on mount/connect).
const snapshotModelStatus = (): void => {
  llmStatus()
    .then(applyLlmStatus)
    .catch((err) => {
      console.error('[assistant] Failed to snapshot model status:', err)
    })
}

// 0.39.39 (#675.1) — push-primary with snapshot-on-connect. When the daemon
// supports broadcasts we take ONE snapshot, then subscribe to
// `llm_status_changed`; the old 2s `setTimeout` poll is gone. The
// `downloadPercent` delta drives the download bar (the Tauri
// `assistant:download-progress` event below still fires too — both converge
// the same store fields, last-write-wins). Against an OLDER / REMOTE daemon
// that doesn't emit the broadcast we KEEP the legacy poll loop so the model
// status still updates.
const pollModelStatus = (): void => {
  llmStatus()
    .then((status) => {
      applyLlmStatus(status)
      if (status.loaded) return
      // Keep polling until loaded (download in flight or still initializing).
      setTimeout(pollModelStatus, 2000)
    })
    .catch((err) => {
      console.error('[assistant] Failed to poll model status:', err)
    })
}

if (serverSupports('daemon-broadcasts')) {
  // Push path: one snapshot now, re-snapshot on every WS (re)connect, and
  // converge on each broadcast.
  setTimeout(snapshotModelStatus, 1000)
  onAppHello(() => snapshotModelStatus())
  onLlmStatusChanged((e) => {
    const store = useAssistantStore.getState()
    if (e.loaded) {
      store.setDownloading(false)
      store.setModelLoaded(true)
    } else if (e.downloading) {
      store.setDownloading(true, e.downloadPercent ?? undefined)
    }
  })
} else {
  // Fallback: legacy 2s poll loop (older / remote daemon, no broadcasts).
  setTimeout(pollModelStatus, 1000)
}

// Listen for download progress events from Tauri backend
// Rust emits: { percent: f64, bytes_downloaded: u64, total_bytes: u64 }
try {
  listen<{ percent: number; bytesDownloaded: number; totalBytes: number }>(
    'assistant:download-progress',
    (event) => {
      const { percent, bytesDownloaded: bytes_downloaded, totalBytes: total_bytes } = event.payload
      const store = useAssistantStore.getState()

      if (percent >= 100 || (total_bytes > 0 && bytes_downloaded >= total_bytes)) {
        store.setDownloading(false)
        store.setModelLoaded(true)
      } else {
        store.setDownloading(true, percent)
      }
    }
  )
} catch {
  // Ignore — not in Tauri environment (e.g., tests)
}
