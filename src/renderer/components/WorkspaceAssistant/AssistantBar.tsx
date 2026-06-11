import { useEffect, useRef, useState, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
// Plan B — the assistant's git tools (stage/commit/worktree/info) are
// host-aware daemon data: route them through the `/cli/git/*` HTTP layer
// (local OR remote) instead of the localhost-pinned Tauri `git_*` invoke
// proxy. GET params snake_case (`path`); POST bodies camelCase. The git
// commands emit NO cross-window sync; `workspace_arrange` (window mgmt)
// stays on Tauri invoke (host-only).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { llmChat } from '@/lib/llmDaemonClient'
import { settingsUpdate } from '@/lib/daemon-settings'
import { useAssistantStore, type DebugPass, type InteractionLogEntry } from '../../stores/assistant'
import { useSettingsStore } from '../../stores/settings'
import { useTabsStore } from '../../stores/tabs'
import { useProjectsStore } from '../../stores/projects'
import { usePanelsStore } from '../../stores/panels'
import { useMergeDialogStore } from '../MergeDialog/MergeDialog'
import { useToastStore } from '../../stores/toast'
import { usePresetsStore, parseCommand } from '../../stores/presets'

interface ToolCall {
  tool: string
  args: Record<string, unknown>
}

interface ChatResponse {
  raw: string
  parsed: { toolCalls?: ToolCall[]; tool_calls?: ToolCall[]; message?: string }
  debugPasses?: DebugPass[]
}

// ── Tool Registry: the complete, finite set of valid tools ──────────

const VALID_TOOLS = new Set([
  'open_terminal',     // Open a terminal in a new tab
  'open_document',     // Open a file in a new tab
  'add_to_pane',       // Add a terminal or document tab to the current pane
  'arrange_layout',    // Split workspace into multiple panes
  'split_window',      // Add a column to the workspace (max 3)
  'unsplit_window',    // Remove the rightmost column
  'resume_chat',       // Resume a past AI conversation
  'switch_workspace',  // Switch to a named workspace
  'ask_agent',         // Delegate a coding task to the default AI agent
  // Git tools
  'stage_all',         // Stage all changes
  'stage_file',        // Stage a specific file
  'unstage_file',      // Unstage a specific file
  'commit',            // Commit staged changes
  'show_diff',         // Open diff view for a file
  'show_changes',      // Open Changes panel
  'merge_branch',      // Open merge dialog for a branch
  'create_worktree',   // Create a new worktree branch
  'ai_commit',         // AI-powered commit: launches fresh Claude to review and commit
  'ai_commit_merge',   // AI-powered commit & merge: commit then merge branch into main
  'update_settings',   // Change an app setting via dot-path
])

// ── Allowed settings paths (whitelist for update_settings tool) ──────
const ALLOWED_SETTINGS_PATHS = new Set([
  'editor.theme', 'editor.fontSize', 'editor.fontFamily', 'editor.tabSize',
  'editor.cursorStyle', 'editor.wordWrap', 'editor.showWhitespace',
  'editor.indentGuides', 'editor.lineNumbers', 'editor.highlightActiveLine',
  'editor.bracketMatching', 'editor.autocomplete', 'editor.foldGutter',
  'editor.vimMode', 'editor.formatOnSave', 'editor.diffStyle',
  'editor.scrollbarAnnotations', 'editor.ligatures', 'editor.minimap',
  'editor.stickyScroll',
  'terminal.fontSize', 'terminal.fontFamily', 'terminal.cursorStyle',
  'terminal.scrollback', 'terminal.naturalTextEditing',
  'sidebarCollapsed', 'leftPanelOpen', 'rightPanelOpen',
  'defaultAgent', 'aiAssistantEnabled', 'agenticSystemsEnabled',
  'claudeAuthAutoRefresh', 'focusGroupsEnabled',
])

interface ChildDescriptor {
  type: 'document' | 'terminal'
  path?: string
  command?: string
  cwd?: string
  items?: ItemDescriptor[]
}

interface ItemDescriptor {
  type: 'document' | 'terminal'
  path?: string
  command?: string
}

/** Validate and sanitize ALL tool calls before execution.
 *  Single source of truth — if a tool isn't handled here, it doesn't execute.
 */
function validateAndSanitize(toolCalls: ToolCall[], cwd: string): ToolCall[] {
  const sanitized: ToolCall[] = []

  for (const call of toolCalls) {
    // REJECT unknown tools — only our registered tools pass
    if (!VALID_TOOLS.has(call.tool)) continue

    const args = { ...call.args }

    // ALWAYS strip cwd — we enforce workspace path
    delete args.cwd

    switch (call.tool) {
      case 'open_terminal': {
        sanitized.push({ tool: call.tool, args })
        break
      }

      case 'open_document': {
        if (!args.path || typeof args.path !== 'string') continue
        sanitized.push({ tool: call.tool, args: { path: args.path } })
        break
      }

      case 'add_to_pane': {
        const type = args.type as string
        if (type !== 'terminal' && type !== 'document') continue
        if (type === 'document' && (!args.path || typeof args.path !== 'string')) continue
        sanitized.push({ tool: call.tool, args: { type, path: args.path, command: args.command } })
        break
      }

      case 'arrange_layout': {
        // Validate direction
        if (args.direction !== 'horizontal' && args.direction !== 'vertical') {
          args.direction = 'horizontal'
        }
        // Sanitize children — inject cwd, validate types
        if (Array.isArray(args.children)) {
          args.children = (args.children as ChildDescriptor[])
            .filter(child => child.type === 'document' || child.type === 'terminal')
            .map(child => {
              const sanitizedChild: ChildDescriptor = {
                type: child.type,
                cwd: child.type === 'terminal' ? cwd : undefined,
                command: child.type === 'terminal' ? child.command : undefined,
                path: child.type === 'document' ? child.path : undefined,
              }
              // Sanitize items array (multiple tabs per pane)
              if (Array.isArray(child.items)) {
                sanitizedChild.items = (child.items as ItemDescriptor[])
                  .filter(item => item.type === 'document' || item.type === 'terminal')
                  .map(item => ({
                    type: item.type,
                    command: item.type === 'terminal' ? item.command : undefined,
                    path: item.type === 'document' ? item.path : undefined,
                  }))
              }
              return sanitizedChild
            })
        }
        sanitized.push({ tool: call.tool, args })
        break
      }

      case 'split_window': {
        // Optional: how many columns total (2 or 3)
        const count = typeof args.count === 'number' ? args.count : undefined
        sanitized.push({ tool: call.tool, args: { count } })
        break
      }

      case 'unsplit_window': {
        sanitized.push({ tool: call.tool, args: {} })
        break
      }

      case 'resume_chat': {
        if (!args.sessionId || typeof args.sessionId !== 'string') continue
        const provider = (args.provider as string) ?? 'claude'
        sanitized.push({ tool: call.tool, args: { provider, sessionId: args.sessionId } })
        break
      }

      case 'switch_workspace': {
        if (!args.name || typeof args.name !== 'string') continue
        sanitized.push({ tool: call.tool, args: { name: args.name } })
        break
      }

      case 'ask_agent': {
        if (!args.query || typeof args.query !== 'string') continue
        sanitized.push({ tool: call.tool, args: { query: args.query } })
        break
      }

      // Git tools
      case 'stage_all':
      case 'show_changes':
      case 'ai_commit':
      case 'ai_commit_merge': {
        sanitized.push({ tool: call.tool, args: call.args ?? {} })
        break
      }

      case 'stage_file':
      case 'unstage_file': {
        if (!args.file || typeof args.file !== 'string') continue
        sanitized.push({ tool: call.tool, args: { file: args.file } })
        break
      }

      case 'commit': {
        if (!args.message || typeof args.message !== 'string') continue
        sanitized.push({ tool: call.tool, args: { message: args.message } })
        break
      }

      case 'show_diff': {
        sanitized.push({ tool: call.tool, args: { file: args.file } })
        break
      }

      case 'merge_branch': {
        if (!args.branch || typeof args.branch !== 'string') continue
        sanitized.push({ tool: call.tool, args: { branch: args.branch } })
        break
      }

      case 'create_worktree': {
        if (!args.branch || typeof args.branch !== 'string') continue
        sanitized.push({ tool: call.tool, args: { branch: args.branch } })
        break
      }

      case 'update_settings': {
        if (!args.path || typeof args.path !== 'string') continue
        if (!ALLOWED_SETTINGS_PATHS.has(args.path as string)) continue
        sanitized.push({ tool: call.tool, args: { path: args.path, value: args.value } })
        break
      }
    }
  }

  return sanitized
}

/** Resolve a potentially relative file path against the workspace root. */
function resolveFilePath(relativePath: string, cwd: string): string {
  if (relativePath.startsWith('/')) return relativePath
  // Strip leading ./ if present
  const clean = relativePath.startsWith('./') ? relativePath.slice(2) : relativePath
  return `${cwd}/${clean}`
}

/** Execute validated tool calls on the tabs store */
async function executeToolCalls(toolCalls: ToolCall[]): Promise<string> {
  const tabsStore = useTabsStore.getState()
  const projectsStore = useProjectsStore.getState()

  const activeProject = projectsStore.projects.find(p => p.id === projectsStore.activeProjectId)
  const cwd = activeProject?.path ?? '~'

  const validCalls = validateAndSanitize(toolCalls, cwd)
  if (validCalls.length === 0) return 'No valid commands found'

  const results: string[] = []

  for (const call of validCalls) {
    try {
      switch (call.tool) {
        case 'arrange_layout': {
          invoke('workspace_arrange', { layout: call.args }).catch((err) => {
            console.error('[AssistantBar] workspace_arrange failed:', err)
          })
          const children = (call.args.children as ChildDescriptor[]) ?? []
          const desc = children.map(c =>
            c.type === 'terminal' ? (c.command ?? 'terminal') : (c.path ?? 'file')
          ).join(' | ')
          results.push(desc)
          break
        }

        case 'open_document': {
          const filePath = resolveFilePath(call.args.path as string, cwd)
          tabsStore.openFileInNewTab(filePath)
          results.push(filePath.split('/').pop() ?? 'file')
          break
        }

        case 'open_terminal': {
          const command = call.args.command as string | undefined
          tabsStore.addTab(cwd, { title: command ?? 'Terminal', command })
          results.push(command ?? 'terminal')
          break
        }

        case 'add_to_pane': {
          const activeTab = tabsStore.getActiveTab()
          if (activeTab) {
            const paneGroupId = tabsStore.getActivePaneGroupId(activeTab.id)
            if (paneGroupId) {
              const itemType = call.args.type as string
              if (itemType === 'terminal') {
                const id = crypto.randomUUID()
                tabsStore.addItemToPaneGroup(activeTab.id, paneGroupId, {
                  id,
                  type: 'terminal',
                  data: { terminalId: id, cwd, command: call.args.command as string | undefined }
                })
                results.push(call.args.command as string ?? 'terminal')
              } else {
                const id = crypto.randomUUID()
                const resolvedPath = resolveFilePath(call.args.path as string, cwd)
                tabsStore.addItemToPaneGroup(activeTab.id, paneGroupId, {
                  id,
                  type: 'file-viewer',
                  data: { filePath: resolvedPath }
                })
                results.push(resolvedPath.split('/').pop() ?? 'file')
              }
            }
          }
          break
        }

        case 'split_window': {
          const targetCount = call.args.count as number | undefined
          if (targetCount) {
            // Split to a specific column count (2 or 3)
            while (tabsStore.splitCount < Math.min(targetCount, 3)) {
              tabsStore.splitTerminalArea(cwd)
            }
            results.push(`${tabsStore.splitCount} columns`)
          } else if (tabsStore.splitCount < 3) {
            tabsStore.splitTerminalArea(cwd)
            results.push(`${tabsStore.splitCount} columns`)
          } else {
            results.push('Already at max columns (3)')
          }
          break
        }

        case 'unsplit_window': {
          if (tabsStore.splitCount > 1) {
            tabsStore.unsplitTerminalArea()
            results.push(tabsStore.splitCount === 1 ? 'Single column' : `${tabsStore.splitCount} columns`)
          } else {
            results.push('Already single column')
          }
          break
        }

        case 'resume_chat': {
          const sessionId = call.args.sessionId as string
          const provider = call.args.provider as string
          if (provider === 'claude') {
            // Include preset flags (e.g. --dangerously-skip-permissions) from user's agent preset
            const presetsState = usePresetsStore.getState()
            const claudePreset = presetsState.presets.find((p) => p.command.split(/\s+/)[0] === 'claude' && p.enabled)
            const presetArgs: string[] = []
            if (claudePreset) {
              const parts = claudePreset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
              const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
              presetArgs.push(...cleaned.slice(1).filter((a: string) => a !== '--resume'))
            }
            tabsStore.addTab(cwd, {
              title: `Claude (resumed)`,
              command: 'claude',
              args: [...presetArgs, '--resume', sessionId]
            })
            results.push('Resumed Claude chat')
          }
          break
        }

        case 'switch_workspace': {
          results.push(`Switch: ${call.args.name}`)
          break
        }

        case 'ask_agent': {
          const query = call.args.query as string
          const agent = useSettingsStore.getState().defaultAgent || 'claude'
          // Launch the default agent with the query as an argument
          tabsStore.addTab(cwd, {
            title: `${agent}: ${query.slice(0, 30)}${query.length > 30 ? '...' : ''}`,
            command: agent,
            args: [query]
          })
          results.push(`Sent to ${agent}`)
          break
        }

        // ── Git tools ──────────────────────────────────────────────

        case 'stage_all': {
          daemonCliPost('git/stage-all', { path: cwd }).catch(console.error)
          results.push('Staged all changes')
          break
        }

        case 'stage_file': {
          const file = call.args.file as string
          daemonCliPost('git/stage', { path: cwd, filePath: file }).catch(console.error)
          results.push(`Staged ${file}`)
          break
        }

        case 'unstage_file': {
          const file = call.args.file as string
          daemonCliPost('git/unstage', { path: cwd, filePath: file }).catch(console.error)
          results.push(`Unstaged ${file}`)
          break
        }

        case 'commit': {
          const message = call.args.message as string
          daemonCliPost('git/commit', { path: cwd, message })
            .then(() => useToastStore.getState().addToast(`Committed: ${message}`, 'success'))
            .catch((e) => useToastStore.getState().addToast(`Commit failed: ${e}`, 'error'))
          results.push(`Committed: ${message}`)
          break
        }

        case 'show_diff': {
          const file = call.args.file as string | undefined
          if (file) {
            const activeTab = tabsStore.getActiveTab()
            if (activeTab) {
              tabsStore.openDiffInPane(activeTab.id, file)
            }
            results.push(`Diff: ${file}`)
          } else {
            // No specific file — activate changes panel
            usePanelsStore.getState().activateTab('changes')
            results.push('Changes panel')
          }
          break
        }

        case 'show_changes': {
          usePanelsStore.getState().activateTab('changes')
          results.push('Changes panel')
          break
        }

        case 'merge_branch': {
          const branch = call.args.branch as string
          const project = projectsStore.projects.find(p => p.id === projectsStore.activeProjectId)
          if (project) {
            const workspace = project.workspaces.find(w => w.branch === branch)
            useMergeDialogStore.getState().show(branch, project.path, project.id, workspace?.id ?? null)
          }
          results.push(`Merge: ${branch}`)
          break
        }

        case 'create_worktree': {
          const branch = call.args.branch as string
          const project = projectsStore.projects.find(p => p.id === projectsStore.activeProjectId)
          if (project) {
            daemonCliPost('git/create-worktree', {
              projectPath: project.path,
              branch,
              projectId: project.id,
            })
              .then(() => {
                useProjectsStore.getState().fetchProjects()
                useToastStore.getState().addToast(`Created worktree: ${branch}`, 'success')
              })
              .catch((e) => useToastStore.getState().addToast(`Failed: ${e}`, 'error'))
          }
          results.push(`Worktree: ${branch}`)
          break
        }

        case 'ai_commit':
        case 'ai_commit_merge': {
          const includeMerge = call.tool === 'ai_commit_merge'
          const defaultAgentId = useSettingsStore.getState().defaultAgent
          const presets = usePresetsStore.getState().presets
          const preset = presets.find((p) => p.id === defaultAgentId)
          if (preset) {
            const { command, args } = parseCommand(preset.command)
            let prompt = (call.args.message as string) || 'Review the changes in this repository and create a well-structured commit with an appropriate commit message.'
            if (includeMerge) {
              prompt += '\n\nAfter committing, merge this branch back into main and resolve any conflicts.'
            }
            tabsStore.addTab(cwd, {
              title: includeMerge ? 'AI Commit & Merge' : 'AI Commit',
              command,
              args: [...args, prompt]
            })
          }
          results.push(includeMerge ? 'AI Commit & Merge' : 'AI Commit')
          break
        }

        case 'update_settings': {
          const settingsPath = call.args.path as string
          const settingsValue = call.args.value

          // Convert dot-path to nested object: "editor.diffStyle" → { editor: { diffStyle: "gutter" } }
          const parts = settingsPath.split('.')
          let update: unknown = settingsValue
          for (let i = parts.length - 1; i >= 0; i--) {
            update = { [parts[i]]: update }
          }

          await settingsUpdate(update as Record<string, unknown>)
          await useSettingsStore.getState().fetchSettings()
          results.push(`Updated ${settingsPath} to ${JSON.stringify(settingsValue)}`)
          break
        }
      }
    } catch {
      results.push(`Failed: ${call.tool}`)
    }
  }

  return results.join(', ')
}

/** Number of recent history items to show in the dropdown. */
const VISIBLE_HISTORY = 5

/** Renders a single debug log entry with expandable raw output. */
function DebugLogEntry({ entry }: { entry: InteractionLogEntry }): React.JSX.Element {
  const [expanded, setExpanded] = useState(false)
  const time = new Date(entry.timestamp).toLocaleTimeString()
  const parsedStr = JSON.stringify(entry.parsed, null, 2)

  return (
    <div
      style={{
        borderBottom: '1px solid #1a1a1a',
        padding: '8px 16px',
      }}
    >
      {/* Header row */}
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full text-left bg-transparent border-none cursor-pointer outline-none"
        style={{ fontFamily: 'inherit', padding: 0 }}
      >
        <div className="flex items-center gap-2">
          <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
            {time}
          </span>
          <span
            className="text-[10px] flex-shrink-0"
            style={{ opacity: 0.5 }}
          >
            {expanded ? '▼' : '▶'}
          </span>
          <span className="text-[11px] text-[var(--color-text-primary)] truncate">
            {entry.message}
          </span>
        </div>
        <div className="flex items-center gap-2 mt-1">
          <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0" style={{ width: '52px' }} />
          <span
            className="text-[10px] truncate"
            style={{
              color: entry.result.startsWith('Error')
                ? '#f87171'
                : 'var(--color-text-muted)',
            }}
          >
            → {entry.result}
          </span>
        </div>
      </button>

      {/* Expanded detail */}
      {expanded && (
        <div className="mt-2 ml-[52px]" style={{ fontSize: '10px' }}>
          {entry.debugPasses.map((pass, pi) => (
            <div key={pi} className="mb-2">
              <div className="text-[var(--color-text-muted)] mb-1">
                Pass {pi + 1} prompt:
              </div>
              <pre
                className="text-[var(--color-text-muted)]"
                style={{
                  background: '#111',
                  padding: '6px 8px',
                  margin: 0,
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-word',
                  maxHeight: '120px',
                  overflowY: 'auto',
                  border: '1px solid #1a1a1a',
                }}
              >
                {pass.prompt}
              </pre>
              <div className="text-[var(--color-text-muted)] mt-1 mb-1">
                Pass {pi + 1} raw output:
              </div>
              <pre
                className="text-[var(--color-text-primary)]"
                style={{
                  background: '#111',
                  padding: '6px 8px',
                  margin: 0,
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-word',
                  maxHeight: '120px',
                  overflowY: 'auto',
                  border: '1px solid #1a1a1a',
                }}
              >
                {pass.rawOutput}
              </pre>
            </div>
          ))}
          <div className="text-[var(--color-text-muted)] mb-1">Parsed result:</div>
          <pre
            className="text-[var(--color-text-primary)]"
            style={{
              background: '#111',
              padding: '6px 8px',
              margin: 0,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
              maxHeight: '150px',
              overflowY: 'auto',
              border: '1px solid #1a1a1a',
            }}
          >
            {parsedStr}
          </pre>
        </div>
      )}
    </div>
  )
}

export default function AssistantBar(): React.JSX.Element | null {
  const isOpen = useAssistantStore((s) => s.isOpen)
  const isLoading = useAssistantStore((s) => s.isLoading)
  const isDownloading = useAssistantStore((s) => s.isDownloading)
  const downloadProgress = useAssistantStore((s) => s.downloadProgress)
  const modelLoaded = useAssistantStore((s) => s.modelLoaded)
  const lastResult = useAssistantStore((s) => s.lastResult)
  const history = useAssistantStore((s) => s.history)
  const interactionLog = useAssistantStore((s) => s.interactionLog)
  const showDebugLog = useAssistantStore((s) => s.showDebugLog)
  const close = useAssistantStore((s) => s.close)
  const setLoading = useAssistantStore((s) => s.setLoading)
  const setLastResult = useAssistantStore((s) => s.setLastResult)
  const addToHistory = useAssistantStore((s) => s.addToHistory)
  const logInteraction = useAssistantStore((s) => s.logInteraction)
  const toggleDebugLog = useAssistantStore((s) => s.toggleDebugLog)
  const clearLog = useAssistantStore((s) => s.clearLog)

  const openSettings = useSettingsStore((s) => s.openSettings)

  const [message, setMessage] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const resultTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // Abort flag: when set, the in-flight response is discarded on arrival
  const abortedRef = useRef(false)

  // History navigation: -1 = not browsing (current draft), 0 = most recent, etc.
  const [historyIndex, setHistoryIndex] = useState(-1)
  // Stash what the user was typing before arrowing into history
  const draftRef = useRef('')
  // Whether to show the history dropdown
  const [showHistory, setShowHistory] = useState(false)

  // Focus input when bar opens; reset state
  useEffect(() => {
    if (isOpen) {
      setMessage('')
      setLastResult(null)
      setHistoryIndex(-1)
      draftRef.current = ''
      setShowHistory(false)
      requestAnimationFrame(() => {
        inputRef.current?.focus()
      })
    }
    return () => {
      if (resultTimerRef.current) {
        clearTimeout(resultTimerRef.current)
        resultTimerRef.current = null
      }
    }
  }, [isOpen, setLastResult])

  // Listen for Escape globally whenever the console is open
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        if (isLoading) {
          abortedRef.current = true
          setLoading(false)
        }
        close()
      }
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [isOpen, isLoading, setLoading, close])

  // Clear lastResult after it's been logged (no auto-close)
  useEffect(() => {
    if (lastResult) {
      resultTimerRef.current = setTimeout(() => {
        setLastResult(null)
      }, 300)
      return () => {
        if (resultTimerRef.current) {
          clearTimeout(resultTimerRef.current)
          resultTimerRef.current = null
        }
      }
    }
  }, [lastResult, setLastResult])

  const handleSubmit = useCallback(async () => {
    const trimmed = message.trim()
    if (!trimmed || isLoading) return

    addToHistory(trimmed)
    setHistoryIndex(-1)
    draftRef.current = ''
    setShowHistory(false)
    abortedRef.current = false
    setLoading(true)
    setMessage('')

    try {
      const activeProject = useProjectsStore.getState().projects.find(
        p => p.id === useProjectsStore.getState().activeProjectId
      )
      const workspacePath = activeProject?.path ?? undefined

      // Check if this is a git repo to enable git tools in the LLM prompt
      let isGitRepo = false
      try {
        if (workspacePath) {
          const info = await daemonCliGet<{ isRepo: boolean }>('git/info', { path: workspacePath })
          isGitRepo = info.isRepo
        }
      } catch { /* not a git repo */ }

      // Phase 2 Unit 2 — `invoke('assistant_chat')` retired; daemon
      // owns /cli/llm/chat and the subprocess supervisor.
      const response = await llmChat({
        message: trimmed,
        workspacePath,
        isGitRepo,
      }) as ChatResponse

      // If the user pressed Escape while we were waiting, discard the result
      if (abortedRef.current) {
        console.log('[AssistantBar] Response discarded (aborted by user)')
        return
      }

      console.log('[AssistantBar] Response:', JSON.stringify(response, null, 2))

      const toolCalls = response.parsed?.toolCalls ?? response.parsed?.tool_calls
      console.log('[AssistantBar] Parsed:', response.parsed, 'Tool calls:', toolCalls)

      let result: string
      if (toolCalls && toolCalls.length > 0) {
        result = await executeToolCalls(toolCalls)
      } else if (response.parsed?.message) {
        result = response.parsed.message
      } else {
        result = response.raw || 'Done'
      }

      setLastResult(result)

      // Log the full interaction for debugging
      logInteraction({
        timestamp: Date.now(),
        message: trimmed,
        result,
        parsed: response.parsed,
        debugPasses: response.debugPasses ?? [],
      })
    } catch (err) {
      if (abortedRef.current) return
      const errorMessage = err instanceof Error ? err.message : String(err)
      const result = `Error: ${errorMessage}`
      setLastResult(result)

      logInteraction({
        timestamp: Date.now(),
        message: trimmed,
        result,
        parsed: null,
        debugPasses: [],
      })
    } finally {
      setLoading(false)
    }
  }, [message, isLoading, setLoading, setLastResult, addToHistory, logInteraction])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        if (showHistory) {
          setShowHistory(false)
        } else {
          close()
        }
        return
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        // If browsing history and user clicks a history item, submit it
        handleSubmit()
        return
      }

      // ── Arrow key history navigation ──────────────────────────
      if (e.key === 'ArrowUp' && history.length > 0) {
        e.preventDefault()
        setShowHistory(true)

        if (historyIndex === -1) {
          // Entering history — stash current draft
          draftRef.current = message
          const idx = 0 // most recent (history is reversed for display)
          setHistoryIndex(idx)
          setMessage(history[history.length - 1])
        } else if (historyIndex < history.length - 1) {
          // Go further back in history
          const next = historyIndex + 1
          setHistoryIndex(next)
          setMessage(history[history.length - 1 - next])
        }
        return
      }

      if (e.key === 'ArrowDown') {
        e.preventDefault()
        if (historyIndex > 0) {
          // Move forward in history (toward recent)
          const next = historyIndex - 1
          setHistoryIndex(next)
          setMessage(history[history.length - 1 - next])
        } else if (historyIndex === 0) {
          // Back to the draft
          setHistoryIndex(-1)
          setMessage(draftRef.current)
          setShowHistory(false)
        }
        return
      }
    },
    [close, handleSubmit, history, historyIndex, message, showHistory]
  )

  // When the user types normally, reset history navigation
  const handleChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    setMessage(e.target.value)
    if (historyIndex !== -1) {
      setHistoryIndex(-1)
      draftRef.current = ''
    }
  }, [historyIndex])

  // Click a history item to fill it in
  const handleHistoryClick = useCallback((cmd: string) => {
    setMessage(cmd)
    setShowHistory(false)
    setHistoryIndex(-1)
    draftRef.current = ''
    inputRef.current?.focus()
  }, [])

  // Auto-scroll ref for console log (must be before conditional return)
  const logEndRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [interactionLog.length, isLoading])

  if (!isOpen) return null

  // Shared bar style
  const barStyle: React.CSSProperties = {
    background: '#111111',
    borderTop: '1px solid var(--color-border)',
    fontFamily: 'var(--font-mono, "MesloLGM Nerd Font", monospace)',
    animation: 'assistantSlideUp 150ms ease-out',
  }

  // Downloading state: show progress bar
  if (isDownloading) {
    return (
      <div
        className="fixed bottom-0 left-0 right-0 z-[900] flex items-center"
        style={{ height: '48px', ...barStyle }}
      >
        <div className="flex items-center gap-3 px-4 w-full">
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
            K2
          </span>
          <div className="flex-1 h-[4px] bg-[#1a1a1a] relative overflow-hidden">
            <div
              className="absolute inset-y-0 left-0 bg-[var(--color-accent)] transition-[width] duration-300"
              style={{ width: `${downloadProgress}%` }}
            />
          </div>
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tabular-nums">
            {Math.round(downloadProgress)}%
          </span>
        </div>
      </div>
    )
  }

  // Model not loaded: show configuration message
  if (!modelLoaded) {
    return (
      <div
        className="fixed bottom-0 left-0 right-0 z-[900] flex items-center"
        style={{ height: '48px', ...barStyle }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            close()
          }
        }}
        tabIndex={-1}
        ref={(el) => el?.focus()}
      >
        <div className="flex items-center gap-3 px-4 w-full">
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
            K2
          </span>
          <span className="text-[11px] text-[var(--color-text-muted)] flex-1">
            AI model not configured
          </span>
          <button
            className="text-[11px] text-[var(--color-accent)] hover:underline bg-transparent border-none cursor-pointer outline-none"
            style={{ fontFamily: 'inherit' }}
            onClick={() => {
              close()
              openSettings('general')
            }}
          >
            Settings
          </button>
          <kbd
            className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)] ml-1"
            style={{ background: '#1a1a1a' }}
          >
            ESC
          </kbd>
        </div>
      </div>
    )
  }

  // Main console UI
  return (
    <div className="fixed bottom-0 left-0 right-0 z-[900]" style={barStyle}>
      {/* ── Debug log panel (expandable raw output) ────────────────── */}
      {showDebugLog && (
        <div
          style={{
            borderBottom: '1px solid var(--color-border)',
            background: '#0a0a0a',
            maxHeight: '50vh',
            overflowY: 'auto',
          }}
        >
          <div className="flex items-center justify-between px-4 py-2" style={{ borderBottom: '1px solid var(--color-border)' }}>
            <span className="text-[11px] text-[var(--color-text-muted)] tracking-wide">
              Debug Log ({interactionLog.length})
            </span>
            {interactionLog.length > 0 && (
              <button
                onClick={clearLog}
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-transparent border-none cursor-pointer outline-none"
                style={{ fontFamily: 'inherit' }}
              >
                Clear
              </button>
            )}
          </div>
          {interactionLog.length === 0 ? (
            <div className="px-4 py-3 text-[11px] text-[var(--color-text-muted)]">
              No interactions yet.
            </div>
          ) : (
            [...interactionLog].reverse().map((entry, i) => (
              <DebugLogEntry key={`${entry.timestamp}-${i}`} entry={entry} />
            ))
          )}
        </div>
      )}

      {/* ── Console log (command → response history) ──────────────── */}
      {!showDebugLog && (
        <div
          style={{
            background: '#0a0a0a',
            minHeight: '120px',
            maxHeight: '200px',
            overflowY: 'auto',
          }}
        >
          {interactionLog.length === 0 && !isLoading && (
            <div className="flex items-center justify-center h-[120px]">
              <span className="text-[11px] text-[var(--color-text-muted)] opacity-30">No commands yet</span>
            </div>
          )}
          {interactionLog.map((entry, i) => {
            const isSystem = entry.message === 'system'
            const isError = (entry.result ?? '').startsWith('Error')
            const isLoaded = (entry.result ?? '').includes('ready')

            if (isSystem) {
              return (
                <div key={`${entry.timestamp}-${i}`} className="px-4 py-1.5">
                  <span className="text-[10px]" style={{ color: isLoaded ? '#22c55e' : 'var(--color-text-muted)', opacity: isLoaded ? 0.7 : 0.4 }}>
                    {isLoaded ? '●' : '○'} {entry.result}
                  </span>
                </div>
              )
            }

            return (
              <div key={`${entry.timestamp}-${i}`} className="px-4 py-1.5">
                <div className="flex items-baseline gap-2">
                  <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0 opacity-40">{'>'}</span>
                  <span className="text-[11px] text-[var(--color-text-muted)]">{entry.message}</span>
                </div>
                <div className="flex items-baseline gap-2 mt-0.5">
                  <span className="text-[10px] flex-shrink-0" style={{ color: isError ? '#f87171' : '#22c55e' }}>
                    {isError ? '✗' : '✓'}
                  </span>
                  <span
                    className="text-[11px]"
                    style={{ color: isError ? '#f87171' : 'var(--color-text-primary)' }}
                  >
                    {entry.result || 'Done'}
                  </span>
                  {isError && !isLoading && (
                    <button
                      className="text-[10px] text-[var(--color-accent)] hover:underline bg-transparent border-none cursor-pointer outline-none ml-1"
                      style={{ fontFamily: 'inherit' }}
                      onClick={() => {
                        setMessage(entry.message)
                        requestAnimationFrame(() => inputRef.current?.focus())
                      }}
                    >
                      retry
                    </button>
                  )}
                </div>
              </div>
            )
          })}
          {isLoading && (
            <div className="px-4 py-1.5">
              <div className="flex items-baseline gap-2">
                <span className="text-[10px] text-[var(--color-text-muted)] flex-shrink-0 opacity-40">{'>'}</span>
                <span className="text-[11px] text-[var(--color-text-muted)]">{history[history.length - 1]}</span>
              </div>
              <div className="flex items-baseline gap-2 mt-0.5">
                <span className="text-[10px] text-[var(--color-accent)] assistant-pulse">⏳</span>
                <span className="text-[11px] text-[var(--color-text-muted)] assistant-pulse">thinking...</span>
              </div>
            </div>
          )}
          <div ref={logEndRef} />
        </div>
      )}

      {/* ── Input bar ─────────────────────────────────────────────── */}
      <div className="flex items-center gap-3 px-4 w-full" style={{ height: '42px' }}>
        <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
          K2
        </span>

        {isLoading ? (
          <>
            <div className="flex-1" />
            <kbd
              className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)]"
              style={{ background: '#1a1a1a', opacity: 0.6 }}
            >
              ESC to cancel
            </kbd>
          </>
        ) : (
          <input
            ref={inputRef}
            type="text"
            value={message}
            onChange={handleChange}
            onKeyDown={handleKeyDown}
            placeholder={
              history.length > 0
                ? 'Type a command...  (↑↓ history)'
                : 'Type a command...'
            }
            spellCheck={false}
            autoComplete="off"
            className="flex-1 bg-transparent text-[12px] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none border-none"
            style={{
              fontFamily: 'inherit',
              background: '#1a1a1a',
              padding: '6px 10px',
              height: '28px'
            }}
          />
        )}

        {!isLoading && (
          <>
            <button
              onClick={handleSubmit}
              disabled={!message.trim()}
              className="text-[10px] px-2 py-1 border border-[var(--color-border)] bg-transparent cursor-pointer outline-none transition-colors"
              style={{
                color: message.trim()
                  ? 'var(--color-text-primary)'
                  : 'var(--color-text-muted)',
                fontFamily: 'inherit',
                opacity: message.trim() ? 1 : 0.5
              }}
            >
              ⏎
            </button>
            <button
              onClick={toggleDebugLog}
              title="Toggle debug log"
              className="text-[10px] px-1.5 py-1 border bg-transparent cursor-pointer outline-none transition-colors"
              style={{
                fontFamily: 'inherit',
                color: showDebugLog ? 'var(--color-accent)' : 'var(--color-text-muted)',
                borderColor: showDebugLog ? 'var(--color-accent)' : 'var(--color-border)',
                opacity: interactionLog.length > 0 ? 1 : 0.4,
              }}
            >
              Log
            </button>
            {interactionLog.length > 0 && (
              <button
                onClick={clearLog}
                title="Clear console"
                className="text-[10px] px-1.5 py-1 border border-[var(--color-border)] bg-transparent cursor-pointer outline-none text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
                style={{ fontFamily: 'inherit' }}
              >
                Clear
              </button>
            )}
            <kbd
              className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)]"
              style={{ background: '#1a1a1a' }}
            >
              ESC
            </kbd>
          </>
        )}
      </div>

      <style>{`
        @keyframes assistantSlideUp {
          from {
            transform: translateY(100%);
            opacity: 0;
          }
          to {
            transform: translateY(0);
            opacity: 1;
          }
        }
        .assistant-pulse {
          animation: assistantPulse 1.5s ease-in-out infinite;
        }
        @keyframes assistantPulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.4; }
        }
      `}</style>
    </div>
  )
}
