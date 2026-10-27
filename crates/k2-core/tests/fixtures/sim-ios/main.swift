import SwiftUI

// K2 sim fixture. Controls sit at fixed fractions of the screen so a HID probe can
// hit them without an accessibility tree:
//   email field (0.5, 0.30) · Save (0.5, 0.45) · Animate (0.5, 0.60)
// Save writes the field's text to Documents/saved.txt (read back with
// `simctl get_app_container <udid> dev.k2.simfixture data`).
@main
struct K2FixtureApp: App {
    var body: some Scene { WindowGroup { ContentView() } }
}

struct ContentView: View {
    @State private var email = ""
    @State private var saved = ""
    @State private var taps = 0
    @State private var animate = false
    var body: some View {
        GeometryReader { g in
            ZStack {
                Text("K2 Fixture").font(.title).accessibilityIdentifier("title")
                    .position(x: g.size.width * 0.5, y: g.size.height * 0.15)
                TextField("Email", text: $email)
                    .textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .accessibilityIdentifier("email")
                    .frame(width: g.size.width * 0.8, height: 44)
                    .position(x: g.size.width * 0.5, y: g.size.height * 0.30)
                Button("Save") {
                    saved = email; taps += 1
                    let url = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                        .appendingPathComponent("saved.txt")
                    try? "\(email)\n".write(to: url, atomically: true, encoding: .utf8)
                }
                .frame(width: 200, height: 60)
                .accessibilityIdentifier("save")
                .position(x: g.size.width * 0.5, y: g.size.height * 0.45)
                Button(animate ? "Stop" : "Animate") { animate.toggle() }
                    .frame(width: 200, height: 60)
                    .accessibilityIdentifier("animate")
                    .position(x: g.size.width * 0.5, y: g.size.height * 0.60)
                Text(saved.isEmpty ? "Not saved" : "Saved \(saved)").accessibilityIdentifier("status")
                    .position(x: g.size.width * 0.5, y: g.size.height * 0.70)
                Text("Taps \(taps)").accessibilityIdentifier("taps")
                    .position(x: g.size.width * 0.5, y: g.size.height * 0.75)
                if animate {
                    TimelineView(.animation) { ctx in
                        let t = ctx.date.timeIntervalSinceReferenceDate
                        Rectangle().fill(.blue).frame(width: 60, height: 60)
                            .rotationEffect(.degrees(t.truncatingRemainder(dividingBy: 2) * 180))
                    }
                    .position(x: g.size.width * 0.5, y: g.size.height * 0.87)
                }
            }
        }
        .ignoresSafeArea()
    }
}
