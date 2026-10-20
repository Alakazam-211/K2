// Settings → Chat Hosting: the K28 panels that wait on S30 (`k2 chat`,
// access + approvals) and S31 (the delivery hub). STUBS ON PURPOSE: they
// render a plain note and make NO daemon calls until those routes exist.
//
// When S30/S31 land, replace each stub with the real panel (S32 PRD §9 as
// amended by plan-sew-s30-s32-on-k2-v1 §1 S32 amends 4–5):
//
// TODO(S30, K28): SewAgentsPanel — bind to `GET /cli/sew/admin/agents`,
//   `POST /cli/sew/admin/agents/{grant,set,revoke}` (fields `space`,
//   `level` read|reply|post|manage, `sendMode`), plus the per-workspace
//   "Manage Sew" switch row (`chat_manage`) through `POST
//   /cli/agent-access/set` (Owner/Admin only; an agent never turns it on
//   for itself).
// TODO(S30, K28): SewApprovalsPanel — `GET /cli/sew/approvals`,
//   `POST /cli/sew/approvals/approve {id, confirmToken}` (vault confirm
//   `chat-approve`, batch `chat-approve-batch`; an agent approval is
//   labelled as an agent's, never person proof), `POST
//   /cli/sew/approvals/deny {id, note}`; card fields `action`, `preview`
//   (plain text, hidden until expanded, untrusted).
// TODO(S31, K28): SewDeliveriesPanel — `GET /cli/sew/admin/deliveries`,
//   `POST /cli/sew/admin/deliveries/{set,reset}`; an agent may set its own
//   (`/cli/sew/chat/deliveries/set`).

import React from 'react'

export const SEW_STUB_NOTE = 'Arrives with k2 chat (S30/S31).'

function Stub({ title, what, testId }: { title: string; what: string; testId: string }): React.JSX.Element {
  return (
    <div className="max-w-xl space-y-2" data-testid={testId}>
      <h3 className="text-sm font-medium text-[var(--color-text-primary)]">{title}</h3>
      <p className="text-[11px] text-[var(--color-text-secondary)]">{what}</p>
      <p className="text-[11px] text-[var(--color-text-muted)]">{SEW_STUB_NOTE}</p>
    </div>
  )
}

/** Agents and their Sew access (level, send mode, Manage Sew). S30. */
export function SewAgentsPanelStub(): React.JSX.Element {
  return (
    <Stub
      testId="sew-stub-agents"
      title="Agents"
      what="Which agents may use Sew, at what level (read, reply, post, manage), and whether their posts wait for approval."
    />
  )
}

/** Queued agent posts waiting for a person (or an agent, per box policy). S30. */
export function SewApprovalsPanelStub(): React.JSX.Element {
  return (
    <Stub
      testId="sew-stub-approvals"
      title="Approvals"
      what="Agent posts that wait for approval land here. Approving needs a person confirm."
    />
  )
}

/** How Sew messages reach each agent (delivery hub). S31. */
export function SewDeliveriesPanelStub(): React.JSX.Element {
  return (
    <Stub
      testId="sew-stub-deliveries"
      title="Deliveries"
      what="How new Sew messages reach each agent: mentions, follows, batching."
    />
  )
}
