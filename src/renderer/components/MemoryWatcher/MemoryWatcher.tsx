import { useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'

interface RendererMemoryStatus {
  resident_bytes: number
  virtual_bytes: number
  pid: number
}

/**
 * 0.38.12 — Renderer memory watcher.
 *
 * Polls the daemon-side `renderer_memory_status` Tauri command every
 * 5 minutes and logs the Tauri process's resident memory to the
 * renderer console (`[k2so/memory]`). The toast warning is gated on
 * *growth above baseline* rather than an absolute threshold, so the
 * resident weight of the local LLM (loaded at app start, ~1+ GB
 * steady-state) doesn't trip a false alarm.
 *
 * Context: C3PO ticket `c9b0d9a9` reported the Tauri WebView
 * resident-memory baseline drifting from ~140 MB → 1+ GB across a
 * day of sustained use, causing macOS RunningBoard to reap the app
 * under Jetsam pressure. WebKit doesn't expose
 * `performance.memory.usedJSHeapSize` (Chromium-only API), so the
 * renderer can't self-measure JS heap directly. The Tauri side has
 * to do it via `proc_pidinfo` and surface the value.
 *
 * Baseline strategy (0.38.13): capture the second sample's RSS as
 * the "settled baseline" (skip the first sample because LLM
 * weights are still loading in early boot), then warn if growth
 * exceeds +800 MB OR absolute hits 3 GB (whichever first). Either
 * signals a real leak, not just LLM resident weight.
 */
const POLL_MS = 5 * 60 * 1000 // 5 minutes
const GROWTH_WARN_BYTES = 800 * 1024 * 1024 // +800 MB above baseline
const ABSOLUTE_WARN_BYTES = 3 * 1024 * 1024 * 1024 // 3 GB hard ceiling
const TOAST_COOLDOWN_MS = 60 * 60 * 1000 // 1 hour between warning toasts

export default function MemoryWatcher(): React.JSX.Element | null {
  const lastWarnAtRef = useRef<number>(0)
  const sampleCountRef = useRef<number>(0)
  const baselineRef = useRef<number | null>(null)
  const addToast = useToastStore((s) => s.addToast)

  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setInterval> | null = null

    const sample = async (): Promise<void> => {
      if (cancelled) return
      try {
        const status = await invoke<RendererMemoryStatus>('renderer_memory_status')
        if (cancelled) return
        const mb = Math.round(status.resident_bytes / 1024 / 1024)
        const vmb = Math.round(status.virtual_bytes / 1024 / 1024)
        sampleCountRef.current += 1

        // Capture baseline at the 2nd sample (skipping #1 because
        // LLM weights may still be loading at boot). After that the
        // baseline is a stable comparison point for growth detection.
        if (sampleCountRef.current === 2 && baselineRef.current === null) {
          baselineRef.current = status.resident_bytes
        }

        const baseline = baselineRef.current
        const growthBytes = baseline === null ? 0 : status.resident_bytes - baseline
        const growthMB = Math.round(growthBytes / 1024 / 1024)
        const baselineLabel =
          baseline === null
            ? 'baseline=pending'
            : `baseline=${Math.round(baseline / 1024 / 1024)}MB growth=${growthMB}MB`
        // eslint-disable-next-line no-console
        console.info(
          `[k2so/memory] pid=${status.pid} rss=${mb}MB vsize=${vmb}MB ${baselineLabel}`
        )

        // Warn iff (growth above baseline > 800 MB) OR (absolute >
        // 3 GB). Either signals a real leak. Pre-baseline samples
        // skip the check — LLM weights may still be loading.
        const growthExceeded =
          baseline !== null && growthBytes > GROWTH_WARN_BYTES
        const absoluteExceeded = status.resident_bytes > ABSOLUTE_WARN_BYTES
        if (
          (growthExceeded || absoluteExceeded) &&
          Date.now() - lastWarnAtRef.current > TOAST_COOLDOWN_MS
        ) {
          lastWarnAtRef.current = Date.now()
          const reason = growthExceeded
            ? `RSS grew ${growthMB} MB above baseline`
            : `RSS hit ${mb} MB`
          addToast(
            `K2 memory: ${reason}. Consider restarting the app to avoid being reaped by macOS.`,
            'error',
            10_000
          )
        }
      } catch (e) {
        // Silent fail — the watcher is non-critical and shouldn't
        // spam the console if the daemon is briefly unreachable.
        // eslint-disable-next-line no-console
        console.debug('[k2so/memory] sample failed:', e)
      }
    }

    // First sample fires 10s after mount so the renderer can finish
    // its initial layout work, workspace hydration, and any deferred
    // popups before we ask the OS for our RSS. 0.39.0 bumped this from
    // 5s to 10s — empirically the first 10s of app launch are the
    // busiest, and the renderer_memory_status invoke was competing
    // with workspace hydration for the Tauri worker pool.
    // Subsequent samples are on the regular interval.
    const startTimer = setTimeout(() => {
      void sample()
      timer = setInterval(() => {
        void sample()
      }, POLL_MS)
    }, 10_000)

    return () => {
      cancelled = true
      clearTimeout(startTimer)
      if (timer) clearInterval(timer)
    }
  }, [addToast])

  return null
}
