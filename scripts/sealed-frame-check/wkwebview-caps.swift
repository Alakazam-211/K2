// Sealed-frame capability check in the system WKWebView (macOS). Loads each
// page from `make-pages.ts` over a `tauri://localhost` custom scheme, with
// the sealed-frame guard registered the way the desktop app registers it
// (WKUserScript, document start, forMainFrameOnly: false) for the `initonly`,
// `both` and `widget` variants, and prints what each frame still has.
// Capability checks only: no page here sets a URL, nothing goes on the network.
//
//   npx vite-node --config vitest.config.ts scripts/sealed-frame-check/make-pages.ts /tmp/k2-sealed
//   swift scripts/sealed-frame-check/wkwebview-caps.swift /tmp/k2-sealed
//
// It also lists the WebRTC / hyperlink-auditing switches this WebKit exposes
// on WKPreferences and WKWebpagePreferences (public or underscore-private),
// for deciding whether an engine-level switch exists. It changes none of them.
// Exits 0 when every guarded variant has WebRTC in the top frame only.

import AppKit
import ObjectiveC
import WebKit

let args = CommandLine.arguments
guard args.count >= 2 else {
    FileHandle.standardError.write("usage: wkwebview-caps.swift <dir>\n".data(using: .utf8)!)
    exit(2)
}
let dir = URL(fileURLWithPath: args[1])
let guardJs = try! String(contentsOf: dir.appendingPathComponent("guard.js"), encoding: .utf8)
let variants = try! String(contentsOf: dir.appendingPathComponent("variants.txt"), encoding: .utf8)
    .split(separator: "\n").map(String.init).filter { !$0.isEmpty }

final class SchemeHandler: NSObject, WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        guard let url = task.request.url,
              let data = try? Data(contentsOf: dir.appendingPathComponent(String(url.path.dropFirst())))
        else {
            task.didFailWithError(NSError(domain: "k2caps", code: 404))
            return
        }
        let resp = HTTPURLResponse(url: url, statusCode: 200, httpVersion: "HTTP/1.1",
                                   headerFields: ["Content-Type": "text/html; charset=utf-8"])!
        task.didReceive(resp)
        task.didReceive(data)
        task.didFinish()
    }
    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

func selectors(_ cls: AnyClass, _ pattern: String) -> [String] {
    var out: [String] = []
    for c: AnyClass in [cls, object_getClass(cls)!] {
        var n: UInt32 = 0
        guard let list = class_copyMethodList(c, &n) else { continue }
        for i in 0..<Int(n) {
            let name = NSStringFromSelector(method_getName(list[i]))
            if name.range(of: pattern, options: [.regularExpression, .caseInsensitive]) != nil { out.append(name) }
        }
        free(list)
    }
    return out.sorted()
}

let switchPattern = "hyperlink|peerconnection|webrtc|mediadevices|mediastream|ice(candidate|servers)"
var report: [String: Any] = [
    "webkitSwitches": [
        "WKPreferences": selectors(WKPreferences.self, switchPattern),
        "WKWebpagePreferences": selectors(WKWebpagePreferences.self, switchPattern),
        "WKWebViewConfiguration": selectors(WKWebViewConfiguration.self, switchPattern),
    ],
]
// Experimental/internal feature flags (`+[WKPreferences _features]`), names only.
let featuresSel = NSSelectorFromString("_features")
if (WKPreferences.self as AnyObject).responds(to: featuresSel),
   let feats = (WKPreferences.self as AnyObject).perform(featuresSel)?.takeUnretainedValue() as? [NSObject] {
    report["webkitFeatureFlags"] = feats.compactMap { f -> String? in
        let key = (f.value(forKey: "key") as? String) ?? ""
        return key.range(of: switchPattern, options: [.regularExpression, .caseInsensitive]) != nil ? key : nil
    }.sorted()
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let window = NSWindow(contentRect: NSRect(x: 40, y: 40, width: 480, height: 320),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "K2 sealed-frame capability check"
window.orderFrontRegardless()

let handler = SchemeHandler()
var results: [[String: Any]] = []
var failures: [String] = []
var current: WKWebView?

func runVariant(_ index: Int) {
    if index >= variants.count {
        report["results"] = results
        report["failures"] = failures
        let data = try! JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        print(String(data: data, encoding: .utf8)!)
        exit(failures.isEmpty ? 0 : 1)
    }
    let v = variants[index]
    let config = WKWebViewConfiguration()
    config.setURLSchemeHandler(handler, forURLScheme: "tauri")
    let baseline = v == "none" || v == "nocsp"
    if !baseline && v != "inframe" {
        config.userContentController.addUserScript(
            WKUserScript(source: guardJs, injectionTime: .atDocumentStart, forMainFrameOnly: false))
    }
    let web = WKWebView(frame: window.contentView!.bounds, configuration: config)
    window.contentView = web
    current = web
    web.load(URLRequest(url: URL(string: "tauri://localhost/\(v).html")!))
    let started = Date()
    Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { timer in
        web.evaluateJavaScript("JSON.stringify(window.__caps || {where: location.href, state: document.readyState, html: document.documentElement.outerHTML.slice(0, 200)})") { value, err in
            let last = (value as? String) ?? String(describing: err)
            guard let text = value as? String,
                  let obj = (try? JSONSerialization.jsonObject(with: text.data(using: .utf8)!)) as? [String: Any],
                  (obj["done"] as? Bool) == true
            else {
                if Date().timeIntervalSince(started) > 20 {
                    timer.invalidate()
                    results.append(["variant": v, "error": "timed out", "last": last])
                    failures.append("\(v): timed out")
                    runVariant(index + 1)
                }
                return
            }
            timer.invalidate()
            results.append(obj)
            if !baseline {
                let frame = (obj["frame"] as? [String: Any]) ?? [:]
                if (obj["top"] as? String) != "function" { failures.append("\(v): top frame lost RTCPeerConnection") }
                for (k, val) in frame where k.hasPrefix("sealed") || k.hasPrefix("child") {
                    if let s = val as? String, s == "function" { failures.append("\(v): \(k) still has it") }
                }
            }
            runVariant(index + 1)
        }
    }
}

DispatchQueue.main.async { runVariant(0) }
app.run()
