// Kept for its callers and tests: the workbook's way in is now one entry in
// LivePreviewEntry's registry (one per type, each behind its own switch).
// Retire this name once nothing imports it.

export { LivePreviewEntry as LiveWorkbookEntry, WATCH_LIVE_LABEL } from './LivePreviewEntry'
