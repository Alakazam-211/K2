import { useCallback, useEffect, useMemo, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { AIFileEditor } from '../AIFileEditor/AIFileEditor'
import { CodeEditor } from '../FileViewerPane/CodeEditor'
import { useResolvedAgentCommand } from '@/hooks/useResolvedAgentCommand'
import { buildEditorAgentArgs } from '@/lib/editor-agent-args'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'

const CATALOG_PACK_SYSTEM_PROMPT = `You are authoring a K2 context catalog pack (library), not AGENTS.md, not a skill.

Edit only pack.toml and layer.md in the current directory.

pack.toml must keep: id, name, description, version, kind = "static", license, author, tags (array). Optional: min_k2, homepage.
Never kind = "live". Never put recommended in tags or toml.
id must stay user:<slug> (or the existing id); reserved prefixes are forbidden (wiki:, manager:, k2:, connections:, heartbeats:, skills:, users:, subagents:, catalog:, pinned:, preset:).

layer.md: lean standing orders, no leading H1, prefer links over dumps, stay under 16 KiB.

This pack is library only until someone stacks it (k2 agent context add <id>).`

interface Props {
  packDir: string
  title: string
  onClose: () => void
}

export function ContextCatalogCreator({ packDir, title, onClose }: Props): React.JSX.Element {
  const packTomlPath = `${packDir.replace(/\/$/, '')}/pack.toml`
  const layerMdPath = `${packDir.replace(/\/$/, '')}/layer.md`
  const [activePath, setActivePath] = useState(packTomlPath)
  const [tomlContent, setTomlContent] = useState('')
  const [layerContent, setLayerContent] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [ready, setReady] = useState(false)

  // Library authoring is host-global — no workspace projectPath.
  const agentCommand = useResolvedAgentCommand(undefined, { scope: 'global' })

  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const [toml, layer] = await Promise.all([
          daemonCliGet<{ content: string }>('fs/read-file', { path: packTomlPath }),
          daemonCliGet<{ content: string }>('fs/read-file', { path: layerMdPath }),
        ])
        if (cancelled) return
        setTomlContent(toml.content)
        setLayerContent(layer.content)
        setReady(true)
      } catch (err) {
        if (cancelled) return
        setError(err instanceof Error ? err.message : String(err))
      }
    }
    void load()
    return () => {
      cancelled = true
    }
  }, [packTomlPath, layerMdPath])

  const handleFileChange = useCallback((content: string, path?: string) => {
    const p = path ?? activePath
    if (p.endsWith('pack.toml')) setTomlContent(content)
    else if (p.endsWith('layer.md')) setLayerContent(content)
  }, [activePath])

  const handleManualRefresh = useCallback(async () => {
    try {
      const [toml, layer] = await Promise.all([
        daemonCliGet<{ content: string }>('fs/read-file', { path: packTomlPath }),
        daemonCliGet<{ content: string }>('fs/read-file', { path: layerMdPath }),
      ])
      setTomlContent(toml.content)
      setLayerContent(layer.content)
    } catch (err) {
      console.error('[context-catalog] refresh failed:', err)
    }
  }, [packTomlPath, layerMdPath])

  const agentPrompt = CATALOG_PACK_SYSTEM_PROMPT

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand) return undefined
    return buildEditorAgentArgs({
      command: agentCommand.command,
      baseArgs: agentCommand.args,
      systemBrief: agentPrompt,
      userMessage:
        'Open pack.toml and layer.md in the current directory. You are authoring a K2 context catalog pack (library). Ask what standing orders this pack should encode, then edit those two files only.',
    })
  }, [agentCommand, agentPrompt])

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-3">
        <p className="text-xs text-[var(--color-status-error-soft)]">Failed to open pack: {error}</p>
        <button
          type="button"
          onClick={onClose}
          className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag"
        >
          &larr; Back to Catalog
        </button>
      </div>
    )
  }

  if (!ready || !agentCommand) {
    return (
      <div className="flex items-center justify-center h-64 text-xs text-[var(--color-text-muted)]">
        {error ? null : !agentCommand ? 'Resolving agent…' : 'Opening pack editor…'}
      </div>
    )
  }

  const previewCode = activePath.endsWith('pack.toml') ? tomlContent : layerContent

  return (
    <AIFileEditor
      filePath={packTomlPath}
      files={[
        { path: packTomlPath, label: 'Metadata' },
        { path: layerMdPath, label: 'Layer' },
      ]}
      watchDir={packDir}
      cwd={packDir}
      command={terminalCommand}
      args={terminalArgs}
      title={`Context pack: ${title}`}
      instructions="Author pack.toml (metadata) and layer.md (standing orders). Library only — this does not stack on a workspace."
      onFileChange={handleFileChange}
      onActiveFileChange={setActivePath}
      onClose={onClose}
      onManualRefresh={handleManualRefresh}
      preview={
        <div className="h-full flex flex-col">
          <div className="flex-shrink-0 px-3 py-1.5 border-b border-[var(--color-border)] text-[10px] text-[var(--color-text-muted)]">
            {activePath.endsWith('pack.toml') ? 'pack.toml' : 'layer.md'}
          </div>
          <div className="flex-1 min-h-0 overflow-hidden">
            {activePath.endsWith('layer.md') ? (
              <div className="h-full overflow-auto p-4">
                <div className="markdown-content">
                  <Markdown remarkPlugins={[remarkGfm]}>
                    {layerContent.trim() || '*Empty layer*'}
                  </Markdown>
                </div>
              </div>
            ) : (
              <CodeEditor
                code={previewCode}
                filePath={activePath}
                onSave={() => {}}
                onChange={() => {}}
                readOnly
              />
            )}
          </div>
        </div>
      }
    />
  )
}
