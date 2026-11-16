// Write the PDF viewer's fixtures as files, for a look in a real viewer or
// the shots harness: `bun src/renderer/test-utils/write-pdf-fixtures.ts <dir>`.
// How they're made: pdf-fixtures.ts (hand-written PDF objects, no library).

import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { drawingPdf, namesOnlyPdf, plainPdf } from './pdf-fixtures'

const dir = process.argv[2]
if (!dir) throw new Error('usage: bun src/renderer/test-utils/write-pdf-fixtures.ts <dir>')
mkdirSync(dir, { recursive: true })
const files: Record<string, Uint8Array> = {
  'drawing.pdf': drawingPdf(),
  'drawing-unused-layers.pdf': drawingPdf({ unusedLayers: ['X-OLD-1', 'X-OLD-2'] }),
  'names-only.pdf': namesOnlyPdf(300),
  'plain.pdf': plainPdf(),
}
for (const [name, bytes] of Object.entries(files)) writeFileSync(join(dir, name), bytes)
console.log(`wrote ${Object.keys(files).join(', ')} to ${dir}`)
