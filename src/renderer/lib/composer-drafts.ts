// Persist in-memory composer drafts across navigation so leaving a ticket
// (or project chat) and returning does not wipe mid-typed text. Module
// scope survives React unmount; not written to disk (session-local).

const ticketDrafts = new Map<string, string>()
const projectChatDrafts = new Map<string, string>()

export function getTicketDraft(id: string): string {
  return ticketDrafts.get(id) ?? ''
}

export function setTicketDraft(id: string, text: string): void {
  if (!id) return
  if (text.length === 0) ticketDrafts.delete(id)
  else ticketDrafts.set(id, text)
}

export function clearTicketDraft(id: string): void {
  ticketDrafts.delete(id)
}

export function getProjectChatDraft(groupId: string): string {
  return projectChatDrafts.get(groupId) ?? ''
}

export function setProjectChatDraft(groupId: string, text: string): void {
  if (!groupId) return
  if (text.length === 0) projectChatDrafts.delete(groupId)
  else projectChatDrafts.set(groupId, text)
}

export function clearProjectChatDraft(groupId: string): void {
  projectChatDrafts.delete(groupId)
}
