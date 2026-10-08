// Zen v2 S0 spike in the system WKWebView, served from a `tauri://localhost`
// custom scheme with the app CSP as a response header: the closest local
// stand-in for the Tauri window (prd-zen-user-widgets-v2 §15). Not signed,
// not the hardened runtime: the decisive run is still the signed app.
//
//   bun scripts/zen-spike-s0/serve.ts --write /tmp/zen-s0
//   swift scripts/zen-spike-s0/wkwebview.swift /tmp/zen-s0 proposed [out.json]
//
// `proposed` picks /tmp/zen-s0/csp-proposed.txt (also v045, r3, tauri, none).
//
// It also runs the REAL renderer bundle: build it with
// `VITE_K2_ZEN_SPIKE=s0 bunx vite build --config vite.config.ts --outDir /tmp/zen-s0-app`,
// copy a csp-<mode>.txt into that folder, and pass it as <dir>.
// Opens a small window for a few seconds, prints the results JSON, exits 0
// when every `proposed` check passes, 1 otherwise.

import AppKit
import WebKit

let args = CommandLine.arguments
guard args.count >= 3 else {
    FileHandle.standardError.write("usage: wkwebview.swift <dir> <csp-mode> [out.json]\n".data(using: .utf8)!)
    exit(2)
}
let dir = URL(fileURLWithPath: args[1])
let mode = args[2]
let outPath = args.count >= 4 ? args[3] : nil
let csp = (try? String(contentsOf: dir.appendingPathComponent("csp-\(mode).txt"), encoding: .utf8)) ?? ""

final class SchemeHandler: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url else { return }
        let name = (url.path.isEmpty || url.path == "/") ? "index.html" : String(url.path.dropFirst())
        guard let data = try? Data(contentsOf: dir.appendingPathComponent(name)) else {
            task.didFailWithError(NSError(domain: "k2spike", code: 404))
            return
        }
        let types = ["js": "text/javascript", "mjs": "text/javascript", "css": "text/css", "json": "application/json",
                     "svg": "image/svg+xml", "png": "image/png", "woff2": "font/woff2", "wasm": "application/wasm"]
        let type = types[(name as NSString).pathExtension.lowercased()] ?? "text/html; charset=utf-8"
        var headers = ["Content-Type": type]
        if !csp.isEmpty { headers["Content-Security-Policy"] = csp }
        let resp = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: headers)!
        task.didReceive(resp)
        task.didReceive(data)
        task.didFinish()
    }
    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let config = WKWebViewConfiguration()
let handler = SchemeHandler()
config.setURLSchemeHandler(handler, forURLScheme: "tauri")
let window = NSWindow(contentRect: NSRect(x: 40, y: 40, width: 640, height: 480),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Zen S0 spike (\(mode))"
let web = WKWebView(frame: window.contentView!.bounds, configuration: config)
window.contentView = web
window.orderFrontRegardless()
web.load(URLRequest(url: URL(string: "tauri://localhost/")!))

let started = Date()
Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { timer in
    web.evaluateJavaScript("JSON.stringify(window.__zenSpikeS0 || null)") { value, _ in
        guard let text = value as? String,
              let data = text.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              (obj["done"] as? Bool) == true
        else {
            if Date().timeIntervalSince(started) > 180 {
                FileHandle.standardError.write("timed out waiting for the spike\n".data(using: .utf8)!)
                exit(3)
            }
            return
        }
        timer.invalidate()
        if let p = outPath { try? text.write(toFile: p, atomically: true, encoding: .utf8) }
        print(text)
        let results = (obj["results"] as? [[String: Any]]) ?? []
        let fails = results.filter {
            ($0["variant"] as? String) == "proposed" && ($0["expect"] as? String) != "info" && ($0["ok"] as? Bool) != true
        }
        exit(fails.isEmpty ? 0 : 1)
    }
}
app.run()
