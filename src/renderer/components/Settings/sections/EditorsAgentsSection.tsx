// Split into EditorsSection + AgentsSection. Kept so any lingering
// imports of EDITORS_AGENTS_MANIFEST / EditorsAgentsSection still resolve.
export { EditorsSection as EditorsAgentsSection } from './EditorsSection'
export { EDITORS_MANIFEST, EDITORS_MANIFEST as EDITORS_AGENTS_MANIFEST } from './EditorsSection'
export { AgentsSection, AGENTS_MANIFEST } from './AgentsSection'
