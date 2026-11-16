// Imported first wherever pdf.js loads (PDFViewer.tsx on the page,
// components/Pdf/pdf.worker.ts in the worker): see map-polyfill.ts.

import { installMapUpsert } from './map-polyfill'

installMapUpsert(Map)
installMapUpsert(WeakMap)
