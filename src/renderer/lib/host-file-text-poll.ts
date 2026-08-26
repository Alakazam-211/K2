// Shared 2s fs/read-file poll for FileViewerPane (text/html/md) and
// Projects HtmlIframePane. Gated on this window's focus and the active
// host's recovery state; in-flight ticks drop on host switch.

import { isHostSwitchedError } from '@/lib/daemon-cli'
import { isRemoteMacTmpPath } from '@/lib/remote-mac-tmp'
import { isConnectionLevelError } from '@/lib/remote-retry'
import { activeHostKey, useConnectHostStore } from '@/stores/connect-host'
import { useWindowFocusStore } from '@/stores/window-focus'

export function shouldSkipHostFilePollTick(): boolean {
  if (!useWindowFocusStore.getState().isFocused) return true
  const s = useConnectHostStore.getState()
  if (s.activeHost !== 'local' && s.recovery.kind !== 'connected') return true
  return false
}

export interface HostFileTextPollOptions {
  filePath: string
  intervalMs: number
  /** Fire one read immediately (HtmlIframePane). FileViewer uses loadFile. */
  immediate?: boolean
  read: () => Promise<string>
  apply: (content: string) => void
  /** Selection guard, dirty buffer, etc. */
  shouldSkip?: () => boolean
  /** Called when the poll stops or a non-host-switch read fails. */
  onError?: (err: unknown) => void
}

/** Start the poll. Returns a stop fn (effect cleanup). */
export function startHostFileTextPoll(opts: HostFileTextPollOptions): () => void {
  let stopped = false
  let gen = 0
  const startedKey = activeHostKey(useConnectHostStore.getState().activeHost)
  let interval: ReturnType<typeof setInterval> | null = null
  let unsubFocus: (() => void) | null = null

  const stop = (): void => {
    if (stopped) return
    stopped = true
    gen += 1
    if (interval !== null) {
      clearInterval(interval)
      interval = null
    }
    if (unsubFocus) {
      unsubFocus()
      unsubFocus = null
    }
  }

  const tick = async (): Promise<void> => {
    if (stopped) return
    if (isRemoteMacTmpPath(opts.filePath)) {
      stop()
      opts.onError?.(new Error('Not available on this server'))
      return
    }
    if (shouldSkipHostFilePollTick()) return
    if (opts.shouldSkip?.()) return
    const thisGen = ++gen
    try {
      const content = await opts.read()
      if (stopped || thisGen !== gen) return
      if (activeHostKey(useConnectHostStore.getState().activeHost) !== startedKey) return
      opts.apply(content)
    } catch (err) {
      if (stopped || thisGen !== gen) return
      if (isHostSwitchedError(err)) {
        stop()
        return
      }
      if (isConnectionLevelError(err)) {
        stop()
        opts.onError?.(err)
        return
      }
      opts.onError?.(err)
    }
  }

  interval = setInterval(() => {
    void tick()
  }, opts.intervalMs)
  unsubFocus = useWindowFocusStore.subscribe((state, prev) => {
    if (stopped) return
    if (!prev.isFocused && state.isFocused) void tick()
  })
  if (opts.immediate) void tick()
  return stop
}
