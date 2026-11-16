// Kept for its callers and tests: the workbook's switch is now one row of
// LivePreviewSwitches (every live view's preview switch, one card).

import { switchFor } from '@/lib/live-view/shared/preview-keys'

export { LivePreviewSwitches as LiveWorkbookPreviewGroup } from './LivePreviewSwitches'

export const LIVE_WORKBOOK_PREVIEW_NAME = switchFor('workbook').name
export const LIVE_WORKBOOK_PREVIEW_LINE = switchFor('workbook').line
