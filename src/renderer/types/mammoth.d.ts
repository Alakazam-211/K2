declare module 'mammoth' {
  interface ConvertResult {
    value: string
    messages: Array<{ type: string; message: string }>
  }

  interface ConvertOptions {
    arrayBuffer?: ArrayBuffer
    buffer?: Buffer
    path?: string
  }

  function convertToHtml(options: ConvertOptions): Promise<ConvertResult>
  function convertToMarkdown(options: ConvertOptions): Promise<ConvertResult>
  function extractRawText(options: ConvertOptions): Promise<ConvertResult>

  export default {
    convertToHtml,
    convertToMarkdown,
    extractRawText,
  }
}
