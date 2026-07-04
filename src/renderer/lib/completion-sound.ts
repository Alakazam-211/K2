// Completion chime — plays ONLY for unseen completions (the active-agents
// store's unseen-done fire), never for every working→idle: if you're
// watching the pane you saw it finish, no noise. Gated by the
// `completionSoundEnabled` setting (Settings → General).
//
// The chime is SYNTHESIZED (Web Audio, two soft sine notes with a fast
// decay — a marimba/glass tap, not an alarm) instead of a bundled audio
// asset: license-clean by construction, works offline, no CSP concerns,
// and no binary blob in the repo.
import { useSettingsStore } from '@/stores/settings'

/** Chime-storm throttle — several agents finishing together (multi-agent
 *  fan-out) produce ONE chime, not a cluster. */
const THROTTLE_MS = 3_000

let _lastPlayedAt = 0
let _ctx: AudioContext | null = null

function note(ctx: AudioContext, freq: number, at: number, dur: number): void {
  const osc = ctx.createOscillator()
  const gain = ctx.createGain()
  osc.type = 'sine'
  osc.frequency.value = freq
  gain.gain.setValueAtTime(0, at)
  gain.gain.linearRampToValueAtTime(0.12, at + 0.015)
  gain.gain.exponentialRampToValueAtTime(0.0001, at + dur)
  osc.connect(gain)
  gain.connect(ctx.destination)
  osc.start(at)
  osc.stop(at + dur + 0.05)
}

export function playCompletionSound(): void {
  if (!useSettingsStore.getState().completionSoundEnabled) return
  const now = Date.now()
  if (now - _lastPlayedAt < THROTTLE_MS) return
  _lastPlayedAt = now
  try {
    _ctx ??= new AudioContext()
    if (_ctx.state === 'suspended') void _ctx.resume()
    const t0 = _ctx.currentTime
    // Two-note tap: A5 → E6, < 0.5s total, quiet (peak gain 0.12).
    note(_ctx, 880, t0, 0.28)
    note(_ctx, 1318.5, t0 + 0.12, 0.34)
  } catch {
    // Audio unavailable (no output device / autoplay policy) — the amber
    // dot still carries the signal.
  }
}

/** Test seam — reset the throttle so each test starts cold. */
export function __resetCompletionSoundThrottleForTests(): void {
  _lastPlayedAt = 0
  _ctx = null
}
