import { useState, useCallback, useEffect, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { AIFileEditor } from '../AIFileEditor/AIFileEditor'
import { CodeEditor } from '../FileViewerPane/CodeEditor'
import { parseCustomThemeJson, serializeThemeToJson, DEFAULT_COLORS, DEFAULT_SYNTAX } from '@/lib/editor-themes'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import type { ThemeColors } from '@/lib/editor-themes'
import type { HighlightStyle } from '@codemirror/language'

// Same preview code used in the Code Editor settings
const PREVIEW_CODE = `import { useState, useEffect, useCallback } from 'react'

// K2 Security Droid — Imperial Data Vault Access Module
// "I find that answer vague and unconvincing." — K2

interface SecurityProtocol {
  clearanceLevel: 'rebel' | 'imperial' | 'classified'
  accessCode: string
  probabilityOfSuccess: number
  isStealthMode?: boolean
}

type MissionStatus = 'infiltrating' | 'compromised' | 'success' | 'told-you-so'

/**
 * Calculates the survival odds for a given mission.
 * Spoiler: they're never good enough for K2's standards.
 */
export function calculateSurvivalOdds(
  crew: string[],
  hasForceSensitive: boolean,
  imperialPresence: number
): { odds: number; commentary: string } {
  const baseOdds = 100 - (imperialPresence * 12.7)
  const crewBonus = crew.length * 3.2
  const forceMultiplier = hasForceSensitive ? 1.47 : 0.89

  const finalOdds = Math.min(97.6, Math.max(0, (baseOdds + crewBonus) * forceMultiplier))

  const commentary =
    finalOdds > 80 ? "Acceptable. I still don't like it." :
    finalOdds > 50 ? "I have a bad feeling about this." :
    finalOdds > 20 ? "Would you like to know the probability of failure?" :
    "I'm not very optimistic about our chances."

  return { odds: Math.round(finalOdds * 100) / 100, commentary }
}

export function useImperialVault(protocol: SecurityProtocol) {
  const [status, setStatus] = useState<MissionStatus>('infiltrating')
  const [dataStolen, setDataStolen] = useState<string[]>([])
  const [alarmTriggered, setAlarmTriggered] = useState(false)

  const sliceTerminal = useCallback(async (terminalId: string) => {
    if (protocol.probabilityOfSuccess < 32.5) {
      setStatus('told-you-so')
      return { success: false, message: "I told you this would happen." }
    }

    try {
      const plans = await fetchClassifiedData(terminalId)
      setDataStolen((prev) => [...prev, ...plans])
      setStatus('success')
      return { success: true, message: "The plans are in the droid." }
    } catch {
      setAlarmTriggered(true)
      setStatus('compromised')
      return { success: false, message: "There are a lot of them." }
    }
  }, [protocol.probabilityOfSuccess])

  useEffect(() => {
    if (!protocol.isStealthMode) return

    const patrol = setInterval(() => {
      const detected = Math.random() > 0.85
      if (detected && !alarmTriggered) {
        setAlarmTriggered(true)
        setStatus('compromised')
        console.warn('[K2] Congratulations. You are being rescued.')
      }
    }, 5000)

    return () => clearInterval(patrol)
  }, [protocol.isStealthMode, alarmTriggered])

  return { status, dataStolen, alarmTriggered, sliceTerminal }
}

async function fetchClassifiedData(id: string): Promise<string[]> {
  const response = await fetch(\`/api/imperial/\${id}/plans\`)
  if (!response.ok) throw new Error('Access denied. Probably.')
  return response.json()
}
`

interface CustomThemeCreatorProps {
  onClose: () => void
  /** The currently active theme ID to use as template base */
  currentThemeId: string
  /** If set, open this existing theme file for editing instead of creating a new one */
  existingThemePath?: string
}

export function CustomThemeCreator({ onClose, currentThemeId, existingThemePath }: CustomThemeCreatorProps): React.JSX.Element {
  const [themePath, setThemePath] = useState<string | null>(null)
  const [themesDir, setThemesDir] = useState<string | null>(null)
  const [themeOverride, setThemeOverride] = useState<{ colors: ThemeColors; highlight: HighlightStyle; isLight?: boolean } | null>(null)
  const [themeVersion, setThemeVersion] = useState(0)
  const closeCreator = useCustomThemesStore((s) => s.closeCreator)

  // Resolve the user's default AI agent command
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  const [error, setError] = useState<string | null>(null)

  // Open existing theme or create new template on mount
  useEffect(() => {
    const init = async () => {
      try {
        const r = await daemonCliGet<{ path: string }>('themes/ensure-dir')
        const dir = r.path
        setThemesDir(dir)

        let path: string
        if (existingThemePath) {
          // Edit an existing custom theme
          path = existingThemePath
        } else {
          // Create a new template
          const baseJson = serializeThemeToJson(
            'My Custom Theme',
            'dark',
            DEFAULT_COLORS,
            DEFAULT_SYNTAX
          )
          const tmpl = await daemonCliPost<{ path: string }>('themes/create-template', { base_theme_json: baseJson })
          path = tmpl.path
        }

        setThemePath(path)

        // Parse the initial file to set preview
        const { content } = await daemonCliGet<{ content: string }>('fs/read-file', { path })
        const parsed = parseCustomThemeJson(content)
        if (parsed) {
          setThemeOverride({ colors: parsed.colors, highlight: parsed.highlight, isLight: parsed.type === 'light' })
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        console.error('[theme-creator] Init failed:', msg)
        setError(msg)
      }
    }
    init()
  }, [currentThemeId, existingThemePath])

  const handleFileChange = useCallback((content: string) => {
    console.log('[theme-creator] handleFileChange called, content length:', content.length, 'first 100 chars:', content.slice(0, 100))
    const parsed = parseCustomThemeJson(content)
    if (parsed) {
      console.log('[theme-creator] Parsed OK:', parsed.name, 'bg:', parsed.colors.bg, 'fg:', parsed.colors.fg, 'keyword:', parsed.colors.accent)
      setThemeOverride({ colors: parsed.colors, highlight: parsed.highlight, isLight: parsed.type === 'light' })
      setThemeVersion((v) => v + 1)
    } else {
      console.log('[theme-creator] Parse returned null — invalid JSON or missing colors.bg/fg')
    }
  }, [])

  // Manual refresh: directly read the file and force update
  const handleManualRefresh = useCallback(async () => {
    if (!themePath && !themesDir) return
    try {
      // Scan for most recent json in the dir
      const entries = await daemonCliGet<{ name: string; path: string; isDirectory: boolean; modifiedAt: number }[]>(
        'fs/read-dir', { path: themesDir! }
      )
      const matching = entries
        .filter((e: any) => !e.isDirectory && e.name.endsWith('.json') && !e.name.startsWith('.'))
        .sort((a: any, b: any) => b.modifiedAt - a.modifiedAt)
      const target = matching[0]
      if (!target) {
        console.log('[theme-creator] Manual refresh: no .json files found in', themesDir)
        return
      }
      console.log('[theme-creator] Manual refresh: reading', target.path)
      const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: target.path })
      const content = result.content
      console.log('[theme-creator] Manual refresh: got content, length:', content.length)
      handleFileChange(content)
    } catch (err) {
      console.error('[theme-creator] Manual refresh failed:', err)
    }
  }, [themePath, themesDir, handleFileChange])

  const handleClose = useCallback(() => {
    closeCreator()
    onClose()
  }, [closeCreator, onClose])

  // Demo diff data for the preview
  const demoChanges = useMemo(() => {
    const m = new Map<number, 'added' | 'modified' | 'deleted'>()
    m.set(11, 'added')
    m.set(14, 'modified')
    m.set(21, 'added')
    m.set(22, 'added')
    m.set(30, 'modified')
    m.set(37, 'added')
    m.set(38, 'added')
    m.set(55, 'deleted')
    m.set(73, 'modified')
    m.set(79, 'added')
    return m
  }, [])

  // Build agent prompt and command args (must be before any conditional returns)
  const agentPrompt = useMemo(() => {
    const fileName = themePath?.split('/').pop() || 'custom-theme.json'
    return `You are a theme designer for K2, a developer workspace app. Your ONLY job is to edit the file "${fileName}" in the current directory. This JSON file defines a code editor theme with two sections: "colors" (editor UI like background, gutter, cursor, selection) and "syntax" (code highlighting for keywords, strings, types, comments, etc). All values are hex color strings. You can also change the "name" field to give the theme a custom name, and set "type" to "dark" or "light". The user sees a live preview that updates each time you save. Ask what kind of theme they want, then iterate. Do NOT edit any other files.`
  }, [themePath])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    const baseArgs = [...agentCommand.args]
    const isClaude = agentCommand.command === 'claude'
    if (isClaude) {
      const fileName = themePath?.split('/').pop() || 'custom-theme.json'
      return [
        ...baseArgs,
        '--append-system-prompt', agentPrompt,
        `Open and read the file ${fileName} in the current directory. This is a K2 editor theme JSON file. The user can see a live preview on the right that updates each time you save. Start by asking them what they'd like to name their theme and what style they're going for (dark, light, warm, cool, vibrant, muted, etc.), then start editing the colors.`,
      ]
    }
    return baseArgs
  }, [agentCommand, agentPrompt, themePath])

  // ── Conditional returns (after all hooks) ──────────────────────────

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-3">
        <p className="text-xs text-red-400">Failed to initialize theme editor: {error}</p>
        <button onClick={onClose} className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag">&larr; Back to Settings</button>
      </div>
    )
  }

  if (!themePath || !themesDir) {
    return (
      <div className="flex items-center justify-center h-64 text-xs text-[var(--color-text-muted)]">
        Setting up theme editor...
      </div>
    )
  }

  return (
    <AIFileEditor
      filePath={themePath}
      watchDir={themesDir}
      cwd={themesDir}
      command={terminalCommand}
      args={terminalArgs}
      title="Theme Creator"
      instructions={`Edit the JSON file to customize your theme. Change colors in "colors" (editor UI) and "syntax" (code highlighting). The preview updates live on each save.`}
      onFileChange={handleFileChange}
      onClose={handleClose}
      onManualRefresh={handleManualRefresh}
      trackFileRename
      preview={
        <div className="h-full">
          <CodeEditor
            key={`theme-preview-${themeVersion}`}
            code={PREVIEW_CODE}
            filePath="preview.tsx"
            onSave={() => {}}
            onChange={() => {}}
            readOnly
            demoLineChanges={demoChanges}
            themeOverride={themeOverride ?? undefined}
          />
        </div>
      }
    />
  )
}
