/**
 * Agent / LLM provider icons.
 *
 * Primary set is vendored from Lobe Icons (MIT) — see
 * `src/renderer/assets/llm-icons/NOTICE.md` for attribution. The Lobe
 * SVGs use `width="1em" height="1em"` and a mix of hardcoded brand
 * fills (claude/codex/gemini/copilot) and `fill="currentColor"` for
 * monochrome marks (cursor/goose/ollama/opencode). We inline the
 * markup via `?raw` so `currentColor` resolves against the wrapper's
 * `color` — set to `--color-text-primary` so monochrome icons stay
 * legible on dark backgrounds and would adapt to a future light theme.
 *
 * Anything not covered by Lobe (aider) falls through to the inline
 * SVG below.
 */

import claudeSvg from '@/assets/llm-icons/claude.svg?raw'
import codexSvg from '@/assets/llm-icons/codex.svg?raw'
import geminiSvg from '@/assets/llm-icons/gemini.svg?raw'
import copilotSvg from '@/assets/llm-icons/copilot.svg?raw'
import cursorSvg from '@/assets/llm-icons/cursor.svg?raw'
import gooseSvg from '@/assets/llm-icons/goose.svg?raw'
import ollamaSvg from '@/assets/llm-icons/ollama.svg?raw'
import opencodeSvg from '@/assets/llm-icons/opencode.svg?raw'
import piSvg from '@/assets/llm-icons/pi.svg?raw'
import interpreterSvg from '@/assets/llm-icons/interpreter.svg?raw'

interface AgentIconProps {
  agent: string
  size?: number
  className?: string
}

// First hit wins, so longer / more specific tokens come before
// generic ones. Matched against `agent.toLowerCase().trim()` —
// callers may pass a preset label ("Claude Code"), a binary name
// ("gemini-cli"), or a provider id ("claude"). String entries match
// as substrings; RegExp entries let short ambiguous tokens like "pi"
// require a word boundary so "API" or "Spike" don't false-match.
const LOBE_ICON_RULES: Array<{ match: string | RegExp; svg: string }> = [
  { match: 'claude', svg: claudeSvg },
  { match: 'codex', svg: codexSvg },
  { match: 'gemini', svg: geminiSvg },
  { match: 'copilot', svg: copilotSvg },
  { match: 'cursor', svg: cursorSvg },
  { match: 'goose', svg: gooseSvg },
  { match: 'ollama', svg: ollamaSvg },
  { match: 'opencode', svg: opencodeSvg },
  { match: 'interpreter', svg: interpreterSvg },
  { match: /\bpi\b/, svg: piSvg },
]

export default function AgentIcon({ agent, size = 14, className = '' }: AgentIconProps): React.JSX.Element {
  const normalized = agent.toLowerCase().trim()

  for (const rule of LOBE_ICON_RULES) {
    const hit = typeof rule.match === 'string'
      ? normalized.includes(rule.match)
      : rule.match.test(normalized)
    if (hit) {
      // Lobe SVGs declare `width="1em" height="1em"`, so setting
      // fontSize on the wrapper sizes the icon. `color` resolves
      // `fill="currentColor"` for the monochrome marks; brand-colored
      // marks ignore it because their fills are hardcoded hex values.
      return (
        <span
          className={className}
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: size,
            height: size,
            fontSize: size,
            color: 'var(--color-text-primary)',
            flexShrink: 0,
          }}
          dangerouslySetInnerHTML={{ __html: rule.svg }}
        />
      )
    }
  }

  const s = size

  switch (normalized) {
    case 'aider':
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <rect x="3" y="3" width="18" height="18" stroke="#14B014" strokeWidth="1.2" fill="none"/>
          <text x="12" y="17" textAnchor="middle" fontFamily="monospace" fontSize="14" fontWeight="bold" fill="#14B014">A</text>
        </svg>
      )

    default:
      // Generic terminal prompt for unknown agents.
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <path d="M7 8L12 12L7 16" stroke="#9CA3AF" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
          <path d="M13 17H18" stroke="#9CA3AF" strokeWidth="1.5" strokeLinecap="round"/>
        </svg>
      )
  }
}
