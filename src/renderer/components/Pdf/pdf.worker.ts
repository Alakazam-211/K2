import '@/lib/pdf/install-map-polyfill'
import 'pdfjs-dist/build/pdf.worker.min.mjs'

// pdf.js's worker, with the Map upsert polyfill installed before it loads
// (a worker is its own global: the page's polyfill doesn't reach it). The
// worker starts itself on this worker's port; usePdfDocument hands that
// port to pdf.js (GlobalWorkerOptions.workerPort).
