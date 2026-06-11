import { useMemo } from 'react'
import { useTimerStore, getElapsedMs, formatElapsed } from '@/stores/timer'

const EXTEND_PRESETS = [
  { label: '5m', ms: 5 * 60 * 1000 },
  { label: '10m', ms: 10 * 60 * 1000 },
  { label: '15m', ms: 15 * 60 * 1000 },
  { label: '30m', ms: 30 * 60 * 1000 },
  { label: '45m', ms: 45 * 60 * 1000 },
  { label: '1h', ms: 60 * 60 * 1000 },
  { label: '2h', ms: 2 * 60 * 60 * 1000 },
  { label: '3h', ms: 3 * 60 * 60 * 1000 },
]

// Built-in flow titles per theme
const BUILT_IN_FLOW_TITLES: Record<string, string[]> = {
  rocket: [
    "Houston, we have a problem... you're too productive.",
    "Re-fueling for another launch.",
    "Orbit achieved. Going interstellar?",
    "The mission isn't over yet, Commander.",
    "Ground control to Major Dev.",
    "Escape velocity reached.",
    "T-minus more code to write.",
  ],
  matrix: [
    "You're beginning to believe.",
    "The Matrix has you... still.",
    "There is no spoon. There is only code.",
    "Free your mind.",
    "Déjà vu. Another round?",
    "Follow the white rabbit.",
    "He's starting to believe.",
  ],
  retro: [
    "INSERT COIN TO CONTINUE",
    "PLAYER 1 — CONTINUE? 9... 8... 7...",
    "HIGH SCORE INCOMING",
    "LEVEL UP!",
    "BONUS ROUND UNLOCKED",
    "GAME OVER? NOT YET.",
    "PRESS START FOR EXTRA LIFE",
  ],
}

const FALLBACK_TITLES = [
  "You're on fire!",
  "Can't stop, won't stop.",
  "Locked in.",
  "Built different.",
  "You didn't hear no bell.",
]

function pickTitle(titles: string[]): string {
  return titles[Math.floor(Math.random() * titles.length)]
}

export default function ExtendTimerDialog(): React.JSX.Element | null {
  const showExtendDialog = useTimerStore((s) => s.showExtendDialog)
  const extendTimer = useTimerStore((s) => s.extendTimer)
  const dismissExtendDialog = useTimerStore((s) => s.dismissExtendDialog)
  const pausedElapsed = useTimerStore((s) => s.pausedElapsed)
  const resumeTime = useTimerStore((s) => s.resumeTime)
  const status = useTimerStore((s) => s.status)
  const countdownTheme = useTimerStore((s) => s.countdownTheme)
  const customThemes = useTimerStore((s) => s.customThemes)

  const title = useMemo(() => {
    // Check built-in themes first
    if (BUILT_IN_FLOW_TITLES[countdownTheme]) {
      return pickTitle(BUILT_IN_FLOW_TITLES[countdownTheme])
    }
    // Check custom themes
    const custom = customThemes.find((t) => t.name === countdownTheme)
    if (custom?.flowTitles && custom.flowTitles.length > 0) {
      return pickTitle(custom.flowTitles)
    }
    return pickTitle(FALLBACK_TITLES)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showExtendDialog])

  if (!showExtendDialog) return null

  const elapsed = getElapsedMs({ status, pausedElapsed, resumeTime })
  const elapsedText = formatElapsed(elapsed)

  return (
    <div className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/60">
      <div className="bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-2xl w-[360px] p-6">
        {/* Header */}
        <div className="text-center mb-5">
          <div className="text-lg font-semibold text-[var(--color-text-primary)] mb-1">
            In a flow state?
          </div>
          <div className="text-xs text-[var(--color-text-muted)] italic">
            {title}
          </div>
        </div>

        {/* Elapsed time */}
        <div className="text-center mb-5">
          <span className="text-2xl font-mono tabular-nums text-red-400 font-semibold">
            {elapsedText}
          </span>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1">elapsed</div>
        </div>

        {/* Add more time */}
        <div className="mb-4">
          <div className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider font-semibold mb-2 text-center">
            Add more time
          </div>
          <div className="flex flex-wrap justify-center gap-2">
            {EXTEND_PRESETS.map((preset) => (
              <button
                key={preset.label}
                onClick={() => extendTimer(preset.ms)}
                className="px-3 py-1.5 text-xs font-mono bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-white/[0.08] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer"
              >
                +{preset.label}
              </button>
            ))}
          </div>
        </div>

        {/* Stop button */}
        <button
          onClick={dismissExtendDialog}
          className="w-full py-2 text-xs font-medium text-red-400 border border-red-400/30 hover:bg-red-400/10 transition-colors cursor-pointer"
        >
          Stop Timer
        </button>
      </div>
    </div>
  )
}
