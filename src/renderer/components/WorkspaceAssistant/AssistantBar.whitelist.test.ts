import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const assistantSrc = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'AssistantBar.tsx'),
  'utf8',
)
const toolsSrc = readFileSync(
  join(
    dirname(fileURLToPath(import.meta.url)),
    '../../../../crates/k2-core/src/llm/tools.rs',
  ),
  'utf8',
)

describe('assistant settings whitelist — no thin-client chrome keys', () => {
  it('strips sidebarCollapsed / leftPanelOpen / rightPanelOpen from ALLOWED_SETTINGS_PATHS', () => {
    const block = assistantSrc.match(
      /ALLOWED_SETTINGS_PATHS = new Set\(\[([\s\S]*?)\]\)/,
    )
    expect(block).toBeTruthy()
    const body = block![1]
    expect(body).not.toMatch(/sidebarCollapsed/)
    expect(body).not.toMatch(/leftPanelOpen/)
    expect(body).not.toMatch(/rightPanelOpen/)
  })

  it('drops the three chrome keys from llm/tools.rs settings path list', () => {
    expect(toolsSrc).not.toMatch(/sidebarCollapsed/)
    expect(toolsSrc).not.toMatch(/leftPanelOpen/)
    expect(toolsSrc).not.toMatch(/rightPanelOpen/)
  })
})
