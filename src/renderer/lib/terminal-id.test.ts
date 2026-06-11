// @vitest-environment jsdom
import { describe, it, expect } from 'vitest'
import {
  agentChatId,
  worktreeChatId,
  heartbeatChatId,
  isLegacyTerminalId,
  isAgentChatId,
  agentNameFromId,
  parseTerminalId,
} from './terminal-id'

describe('terminal-id builders', () => {
  it('builds agent chat id (0.37.5: bare project_id, agent ignored)', () => {
    // Post-0.37.5: agent name is no longer part of the canonical
    // shape. The agent argument is kept for back-compat at call
    // sites but produces the bare-pid form. Reverting agentChatId
    // to the pre-0.37.5 prefix shape MUST flip this assertion to
    // "FAIL".
    expect(agentChatId('p_abc', 'manager')).toBe('agent-chat:p_abc')
    expect(agentChatId('p_abc', 'scout')).toBe('agent-chat:p_abc')
  })

  it('builds worktree id', () => {
    expect(worktreeChatId('ws_xyz')).toBe('agent-chat:wt:ws_xyz')
  })

  it('builds heartbeat id (heartbeat keying unchanged in 0.37.5)', () => {
    expect(heartbeatChatId('p_abc', 'manager', 'triage')).toBe('agent-chat:p_abc:manager:hb:triage')
  })
})

describe('parseTerminalId — namespaced forms', () => {
  it('parses bare-pid agent chat (0.37.5)', () => {
    expect(parseTerminalId('agent-chat:p_abc')).toEqual({
      kind: 'agent_chat',
      projectId: 'p_abc',
      agent: '',
    })
  })

  it('parses legacy <pid>:<agent> agent chat (pre-0.37.5, still recognized for back-compat)', () => {
    expect(parseTerminalId('agent-chat:p_abc:manager')).toEqual({
      kind: 'agent_chat',
      projectId: 'p_abc',
      agent: 'manager',
    })
  })

  it('parses heartbeat with hyphenated names', () => {
    expect(parseTerminalId('agent-chat:p_xyz:pod-leader:hb:daily-brief')).toEqual({
      kind: 'heartbeat_chat',
      projectId: 'p_xyz',
      agent: 'pod-leader',
      heartbeat: 'daily-brief',
    })
  })

  it('parses worktree', () => {
    expect(parseTerminalId('agent-chat:wt:ws_abc')).toEqual({
      kind: 'worktree',
      workspaceId: 'ws_abc',
    })
  })
})

describe('parseTerminalId — legacy forms', () => {
  it('parses legacy unscoped agent chat', () => {
    expect(parseTerminalId('agent-chat-manager')).toEqual({
      kind: 'legacy_agent_chat',
      agent: 'manager',
    })
  })

  it('parses legacy worktree', () => {
    expect(parseTerminalId('agent-chat-wt-ws_abc')).toEqual({
      kind: 'legacy_worktree',
      workspaceId: 'ws_abc',
    })
  })
})

describe('parseTerminalId — rejection', () => {
  it('rejects non-agent ids', () => {
    expect(parseTerminalId('term-7')).toBeNull()
    expect(parseTerminalId('')).toBeNull()
  })

  it('rejects malformed ids', () => {
    expect(parseTerminalId('agent-chat:')).toBeNull()
    expect(parseTerminalId('agent-chat:wt:')).toBeNull()
    // 0.37.5: `agent-chat:p_abc:` (trailing colon, empty agent
    // tail) was previously rejected; now we treat the bare-pid
    // form as canonical. The trailing-colon variant still rejects
    // because the parser sees a non-empty head + empty tail and
    // can't classify.
    expect(parseTerminalId('agent-chat:p_abc:')).toBeNull()
    expect(parseTerminalId('agent-chat:p_abc:manager:hb:')).toBeNull()
    expect(parseTerminalId('agent-chat:p_abc::hb:triage')).toBeNull()
    expect(parseTerminalId('agent-chat-')).toBeNull()
    expect(parseTerminalId('agent-chat-wt-')).toBeNull()
  })
})

describe('isLegacyTerminalId', () => {
  it('recognises both legacy forms', () => {
    expect(isLegacyTerminalId('agent-chat-manager')).toBe(true)
    expect(isLegacyTerminalId('agent-chat-wt-ws_abc')).toBe(true)
  })

  it('rejects new form', () => {
    expect(isLegacyTerminalId('agent-chat:p_abc:manager')).toBe(false)
    expect(isLegacyTerminalId('agent-chat:wt:ws_abc')).toBe(false)
  })

  it('rejects unrelated ids', () => {
    expect(isLegacyTerminalId('term-7')).toBe(false)
    expect(isLegacyTerminalId('')).toBe(false)
  })
})

describe('isAgentChatId', () => {
  it('matches all agent-chat shapes new and legacy', () => {
    expect(isAgentChatId('agent-chat:p_abc:manager')).toBe(true)
    expect(isAgentChatId('agent-chat:p_abc:manager:hb:triage')).toBe(true)
    expect(isAgentChatId('agent-chat:wt:ws_abc')).toBe(true)
    expect(isAgentChatId('agent-chat-manager')).toBe(true)
    expect(isAgentChatId('agent-chat-wt-ws_abc')).toBe(true)
  })

  it('rejects non-agent ids', () => {
    expect(isAgentChatId('term-7')).toBe(false)
    expect(isAgentChatId('')).toBe(false)
  })
})

describe('agentNameFromId', () => {
  it('extracts agent from new agent_chat id', () => {
    expect(agentNameFromId('agent-chat:p_abc:manager')).toBe('manager')
  })

  it('extracts agent from heartbeat id', () => {
    expect(agentNameFromId('agent-chat:p_abc:pod-leader:hb:triage')).toBe('pod-leader')
  })

  it('extracts agent from legacy unscoped id', () => {
    expect(agentNameFromId('agent-chat-manager')).toBe('manager')
  })

  it('returns null for worktree forms (no agent name in id)', () => {
    expect(agentNameFromId('agent-chat:wt:ws_abc')).toBeNull()
    expect(agentNameFromId('agent-chat-wt-ws_abc')).toBeNull()
  })

  it('returns null for non-agent ids', () => {
    expect(agentNameFromId('term-7')).toBeNull()
  })
})

describe('round trip', () => {
  it('agent_chat round trip (0.37.5: agent dropped from canonical id)', () => {
    // Post-0.37.5: agentChatId ignores the agent arg and produces
    // the bare-pid form. The parser yields `agent: ''` for that
    // shape (see the bare-pid test at line ~34). The legacy
    // `<pid>:<agent>` shape still round-trips with the agent name
    // intact — covered separately below.
    const id = agentChatId('p_1', 'alice')
    const parsed = parseTerminalId(id)
    expect(parsed).toEqual({ kind: 'agent_chat', projectId: 'p_1', agent: '' })
  })

  it('heartbeat round trip', () => {
    const id = heartbeatChatId('p_1', 'manager', 'triage')
    const parsed = parseTerminalId(id)
    expect(parsed).toEqual({
      kind: 'heartbeat_chat',
      projectId: 'p_1',
      agent: 'manager',
      heartbeat: 'triage',
    })
  })

  it('worktree round trip', () => {
    const id = worktreeChatId('ws_xyz')
    const parsed = parseTerminalId(id)
    expect(parsed).toEqual({ kind: 'worktree', workspaceId: 'ws_xyz' })
  })
})
