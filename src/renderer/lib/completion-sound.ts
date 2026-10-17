// Completion chime — plays ONLY for unseen completions (the active-agents
// store's unseen-done fire), never for every working→idle: if you're
// watching the pane you saw it finish, no noise. AND-gated by the
// global `completionSoundEnabled` setting (Settings → General) and the
// per-workspace `projects.completionSoundEnabled` flag (default ON).
//
// Every chime is SYNTHESIZED (Web Audio oscillators with fast decays)
// instead of a bundled audio asset: license-clean by construction, works
// offline, no CSP concerns, and no binary blob in the repo.
//
// 0.46.1: which chime (`completionSound`) and how loud
// (`completionSoundVolume`, 0–100) are global daemon settings, so every
// client — local or remote — plays the same sound at the same level.
import { useSettingsStore } from '@/stores/settings'
import {
  clampCompletionSoundVolume,
  completionSoundGain,
  normalizeCompletionSound,
  type CompletionSoundId,
} from './completion-sound-options'

/** The fields of a project record the chime reads. */
export interface ChimeProject {
  id: string
  completionSoundEnabled?: number
}

/** One synthesized note. `peak` is the gain at volume 50. */
interface Note {
  freq: number
  /** Glide to this pitch over the note (the pop). */
  freqEnd?: number
  at: number
  dur: number
  peak: number
  attack?: number
  type?: OscillatorType
}

/** The sounds, as notes. `chime` is the pre-0.46.1 sound, unchanged. */
const SOUND_NOTES: Record<CompletionSoundId, Note[]> = {
  // Two-note tap: A5 → E6, < 0.5s total, quiet (peak gain 0.12).
  chime: [
    { freq: 880, at: 0, dur: 0.28, peak: 0.12 },
    { freq: 1318.5, at: 0.12, dur: 0.34, peak: 0.12 },
  ],
  // A slower, lower rising triad (C5 E5 G5) with a soft attack.
  soft: [
    { freq: 523.25, at: 0, dur: 0.5, peak: 0.09, attack: 0.03 },
    { freq: 659.25, at: 0.09, dur: 0.5, peak: 0.09, attack: 0.03 },
    { freq: 783.99, at: 0.18, dur: 0.6, peak: 0.09, attack: 0.03 },
  ],
  // A short downward pitch drop — a bubble pop.
  pop: [{ freq: 620, freqEnd: 180, at: 0, dur: 0.12, peak: 0.2, attack: 0.005 }],
  // A single bell: C6 with a quieter inharmonic partial that dies first.
  ding: [
    { freq: 1046.5, at: 0, dur: 0.9, peak: 0.11, attack: 0.005 },
    { freq: 2888.3, at: 0, dur: 0.35, peak: 0.03, attack: 0.005 },
  ],
  // A very short high click.
  tick: [{ freq: 2000, at: 0, dur: 0.04, peak: 0.08, attack: 0.002, type: 'triangle' }],
}

/** Chime-storm throttle — several agents finishing together (multi-agent
 *  fan-out) produce ONE chime, not a cluster. */
const THROTTLE_MS = 3_000

let _lastPlayedAt = 0
let _ctx: AudioContext | null = null

function playNote(ctx: AudioContext, n: Note, t0: number, gainMul: number): void {
  const osc = ctx.createOscillator()
  const gain = ctx.createGain()
  const at = t0 + n.at
  const peak = n.peak * gainMul
  osc.type = n.type ?? 'sine'
  if (n.freqEnd !== undefined) {
    osc.frequency.setValueAtTime(n.freq, at)
    osc.frequency.exponentialRampToValueAtTime(n.freqEnd, at + n.dur)
  } else {
    osc.frequency.value = n.freq
  }
  gain.gain.setValueAtTime(0, at)
  gain.gain.linearRampToValueAtTime(peak, at + (n.attack ?? 0.015))
  // The decay target must sit below the peak even at the lowest volumes.
  gain.gain.exponentialRampToValueAtTime(Math.min(0.0001, peak / 100), at + n.dur)
  osc.connect(gain)
  gain.connect(ctx.destination)
  osc.start(at)
  osc.stop(at + n.dur + 0.05)
}

/** Play `sound` at `volume` now — no gates, no throttle. Volume 0 never
 *  touches audio. Audio errors (no output device, autoplay policy) are
 *  dropped: the amber dot still carries the signal. */
function playSound(sound: CompletionSoundId, volume: number): void {
  const gainMul = completionSoundGain(volume)
  if (gainMul <= 0) return
  try {
    _ctx ??= new AudioContext()
    if (_ctx.state === 'suspended') void _ctx.resume()
    const t0 = _ctx.currentTime
    for (const n of SOUND_NOTES[sound]) playNote(_ctx, n, t0, gainMul)
  } catch {
    // Audio unavailable — see above.
  }
}

/** Settings → Preview: the chosen sound at the chosen volume, even when
 *  the completion sound is switched off, and outside the chime throttle. */
export function previewCompletionSound(sound: unknown, volume: unknown): void {
  playSound(normalizeCompletionSound(sound), clampCompletionSoundVolume(volume))
}

function workspaceSoundEnabled(projectId: string | null | undefined, projects: readonly ChimeProject[]): boolean {
  if (!projectId) return true
  const project = projects.find((p) => p.id === projectId)
  // Missing field / unknown project → treat as ON (column default 1).
  return (project?.completionSoundEnabled ?? 1) !== 0
}

/** `projectId` is the fire-time pane bind. Null/undefined → global gate only.
 *  `projects` is the project list of the ROOM the pane is in (MS21/MS68): the
 *  workspace mute is read from that server's record, never looked up by id
 *  in another server's list. */
export function playCompletionSound(
  projectId: string | null | undefined,
  projects: readonly ChimeProject[],
): void {
  const settings = useSettingsStore.getState()
  if (!settings.completionSoundEnabled) return
  // Workspace mute is checked BEFORE the 3s throttle so a muted
  // workspace finishing does not swallow a later unmuted chime.
  if (!workspaceSoundEnabled(projectId, projects)) return
  const now = Date.now()
  if (now - _lastPlayedAt < THROTTLE_MS) return
  _lastPlayedAt = now
  playSound(
    normalizeCompletionSound(settings.completionSound),
    clampCompletionSoundVolume(settings.completionSoundVolume),
  )
}

/** Test seam — reset the throttle so each test starts cold. */
export function __resetCompletionSoundThrottleForTests(): void {
  _lastPlayedAt = 0
  _ctx = null
}
