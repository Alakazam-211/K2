// The ring of an incoming DM call (prd-home-people-channels-v1 §11a.3): two
// soft tones, every two seconds, made with Web Audio (no sound file ships).
// A silent ring never starts it. Where Web Audio is missing (tests, an old
// webview) it does nothing.

const TONES_HZ = [660, 880] as const
const EVERY_MS = 2_000

let ctx: AudioContext | null = null
let timer: ReturnType<typeof setInterval> | null = null

function beep(ac: AudioContext): void {
  TONES_HZ.forEach((hz, i) => {
    const osc = ac.createOscillator()
    const gain = ac.createGain()
    const start = ac.currentTime + i * 0.22
    osc.frequency.value = hz
    gain.gain.setValueAtTime(0, start)
    gain.gain.linearRampToValueAtTime(0.08, start + 0.02)
    gain.gain.linearRampToValueAtTime(0, start + 0.2)
    osc.connect(gain).connect(ac.destination)
    osc.start(start)
    osc.stop(start + 0.21)
  })
}

/** Start ringing (idempotent). */
export function startRingSound(): void {
  if (timer !== null) return
  const AC = (globalThis as { AudioContext?: typeof AudioContext }).AudioContext
  if (!AC) return
  try {
    ctx = ctx ?? new AC()
    const ac = ctx
    beep(ac)
    timer = setInterval(() => beep(ac), EVERY_MS)
  } catch (err) {
    console.warn('[home-chat] ring sound unavailable:', err)
  }
}

/** Stop ringing (idempotent). */
export function stopRingSound(): void {
  if (timer !== null) clearInterval(timer)
  timer = null
}
