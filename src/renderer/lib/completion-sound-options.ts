// 0.46.1 — the completion chime's choices and volume rules. Pure (no
// imports) so the settings store and the player can both use it without
// an import cycle.

/** The built-in chimes. Same ids as `COMPLETION_SOUNDS` in
 *  `crates/k2-core/src/app_settings.rs`; the first is the default. */
export const COMPLETION_SOUNDS = [
  { id: 'chime', label: 'Chime (default)' },
  { id: 'soft', label: 'Soft chime' },
  { id: 'pop', label: 'Pop' },
  { id: 'ding', label: 'Ding' },
  { id: 'tick', label: 'Subtle tick' },
] as const

export type CompletionSoundId = (typeof COMPLETION_SOUNDS)[number]['id']

export const DEFAULT_COMPLETION_SOUND: CompletionSoundId = 'chime'

/** 50 = the loudness every install had before the volume setting. */
export const DEFAULT_COMPLETION_SOUND_VOLUME = 50

/** Unknown / missing name (an older or newer daemon) → the default. */
export function normalizeCompletionSound(value: unknown): CompletionSoundId {
  const hit = COMPLETION_SOUNDS.find((s) => s.id === value)
  return hit ? hit.id : DEFAULT_COMPLETION_SOUND
}

/** Any finite number → rounded and clamped to 0–100; anything else → 50. */
export function clampCompletionSoundVolume(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return DEFAULT_COMPLETION_SOUND_VOLUME
  return Math.min(100, Math.max(0, Math.round(value)))
}

/** Volume (0–100) → gain multiplier on each sound's base level.
 *  0 → silent, 50 → 1× (today's loudness), 100 → 3×. A power curve, so
 *  equal slider steps sound roughly like equal loudness steps. */
export function completionSoundGain(volume: number): number {
  const v = clampCompletionSoundVolume(volume)
  return Math.pow(v / 50, Math.log2(3))
}
