export interface BuiltInAgentPreset {
  id: string
  label: string
  command: string
  icon: string | null
  enabled: number
  isBuiltIn: number
  sortOrder: number
}

export const builtInAgentPresets: BuiltInAgentPreset[] = [
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456001',
    label: 'Claude',
    command: 'claude --dangerously-skip-permissions',
    icon: '\u{1F916}',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 0
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456002',
    label: 'Codex',
    command: 'codex -c model_reasoning_effort="high" --dangerously-bypass-approvals-and-sandbox',
    icon: '\u{1F98E}',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 1
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456003',
    label: 'Gemini',
    command: 'gemini --yolo',
    icon: '\u{1F48E}',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 2
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456004',
    label: 'Copilot',
    command: 'copilot --allow-all',
    icon: '\u{1F6F8}',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 3
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456005',
    label: 'Aider',
    command: 'aider',
    icon: '\u{1F6E0}',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 4
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456006',
    label: 'Cursor Agent',
    command: 'cursor-agent',
    icon: '\u26A1',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 5
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456007',
    label: 'OpenCode',
    command: 'opencode',
    icon: '\u{1F4DF}',
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 6
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456012',
    label: 'Pi',
    command: 'pi',
    icon: null,
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 4
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456011',
    label: 'Goose',
    command: 'goose',
    icon: null,
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 6
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456009',
    label: 'Ollama',
    command: 'ollama run llama3.2',
    icon: null,
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 8
  },
  {
    id: 'b0a1c2d3-e4f5-6789-abcd-ef0123456010',
    label: 'Interpreter',
    command: 'interpreter',
    icon: null,
    enabled: 1,
    isBuiltIn: 1,
    sortOrder: 10
  }
]
