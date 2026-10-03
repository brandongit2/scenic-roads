// Scenic's menu bar item: the build agent's state at a glance (docs/plan.md §8, Status). An icon for
// the state (building, paused, waiting, nothing to do, a problem, the build Mac out of touch), a
// menu with the details, and a notification for every change. It asks the map's server on this Mac
// (`/api/build`), which answers with this Mac's own agent's status when the agent runs here and
// with the heartbeat the agent copies to the NAS otherwise.
//
// The launcher runs it (`scenic-launcher status`, from ~/Library/LaunchAgents/local.scenic.status.plist)
// from the installed app; it quits when a newer app is installed, and the launcher starts that one.
//
//   swiftc -O -swift-version 5 -o Scenic.app/Contents/MacOS/scenic-status tools/status/main.swift
//   scenic-status --print                   the icon and menu for the status now, as text
//   scenic-status --replay a.json b.json …  the notifications a sequence of answers would send
// SCENIC_STATUS_SERVER overrides the server (http://127.0.0.1:8080).

import AppKit
import CoreServices
import UserNotifications

let server = URL(string: ProcessInfo.processInfo.environment["SCENIC_STATUS_SERVER"] ?? "http://127.0.0.1:8080")!
let home = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/scenic")
/// Without a heartbeat for this long, the build Mac counts as out of touch (asleep, off, away).
let outOfTouch = 15 * 60

// What /api/build answers (the agent's status, crates/pipeline/src/agent/mod.rs Status).
struct Reply: Decodable {
    let status: Status?
    let local: Bool
    let now: Int
    let log: String?
}

struct Status: Decodable {
    let host: String
    let app: String?
    let beat: Int
    let conditions: Conditions
    let job: Job?
    let waiting: [Waiting]
    let recent: [Done]
    let built: [String: Built]?
}

struct Conditions: Decodable {
    let ac: Bool
    let nas: Bool
    let battery: Int?
}

struct Job: Decodable {
    let id: String
    let what: String
    let started: Int
    let paused: String?
    let tail: String?
}

struct Waiting: Decodable {
    let what: String
    let why: String
}

struct Done: Decodable {
    let id: String
    let what: String
    let ok: Bool
    let ended: Int
    let secs: Int
    let note: String?
}

struct Built: Decodable {
    let built: Int
    let total: Int
}

enum Kind {
    case unknown, outOfTouch, building, paused, waiting, idle, problem

    var symbol: String {
        switch self {
        case .unknown: return "questionmark.circle"
        case .outOfTouch: return "moon.zzz"
        case .building: return "gearshape.fill"
        case .paused: return "pause.circle"
        case .waiting: return "circle.dotted"
        case .idle: return "checkmark.circle"
        case .problem: return "exclamationmark.triangle"
        }
    }
}

/// "45 s", "12 min", "3 h 5 min", "2 days".
func duration(_ secs: Int) -> String {
    if secs < 60 { return "\(max(secs, 0)) s" }
    if secs < 3600 { return "\(secs / 60) min" }
    if secs < 2 * 86400 {
        let m = secs / 60 % 60
        return m == 0 ? "\(secs / 3600) h" : "\(secs / 3600) h \(m) min"
    }
    return "\(secs / 86400) days"
}

func clock(_ t: Int) -> String {
    let f = DateFormatter()
    f.dateFormat = Calendar.current.isDateInToday(Date(timeIntervalSince1970: TimeInterval(t))) ? "HH:mm" : "d MMM HH:mm"
    return f.string(from: Date(timeIntervalSince1970: TimeInterval(t)))
}

func clip(_ s: String, _ n: Int = 80) -> String {
    s.count <= n ? s : String(s.prefix(n - 1)) + "…"
}

/// The state, with a line saying it.
func classify(_ r: Reply?) -> (Kind, String) {
    guard let r = r else { return (.unknown, "The map's server on this Mac isn't answering") }
    guard let s = r.status else { return (.unknown, "No word from the build Mac (is the NAS reachable?)") }
    if r.now - s.beat > outOfTouch { return (.outOfTouch, "Build Mac out of touch since \(clock(s.beat))") }
    if let j = s.job {
        if j.paused != nil { return (.paused, "Paused") }
        return (.building, "Building")
    }
    if let w = s.waiting.first(where: { $0.why.contains("failed") }) { return (.problem, clip("Failed: \(w.what)")) }
    if let d = s.recent.first, !d.ok { return (.problem, clip("Failed: \(d.what)")) }
    if let w = s.waiting.first { return (.waiting, clip("Waiting: \(w.what)")) }
    return (.idle, "Nothing to build")
}

/// One line of the menu: plain, small and dim, or the log's monospace.
enum Style {
    case title, plain, small, mono, header, separator
}

struct Line {
    let text: String
    let style: Style
}

/// The menu's lines for an answer, under the state's line.
func lines(_ r: Reply?, _ line: String) -> [Line] {
    var out = [Line(text: line, style: .title)]
    guard let r = r, let s = r.status else { return out }
    if let j = s.job {
        out.append(Line(text: clip(j.what), style: .plain))
        if let p = j.paused { out.append(Line(text: clip(p), style: .small)) }
        out.append(Line(text: "Running \(duration(r.now - j.started)) (since \(clock(j.started)))", style: .small))
        // The log's last lines, without the terminal's colour codes.
        let plain = (j.tail ?? "").replacingOccurrences(of: "\u{1B}\\[[0-9;]*[A-Za-z]", with: "", options: .regularExpression)
        for l in plain.split(separator: "\n").suffix(3) where !l.trimmingCharacters(in: .whitespaces).isEmpty {
            out.append(Line(text: clip(String(l).trimmingCharacters(in: .whitespaces), 90), style: .mono))
        }
    }
    let power = s.conditions.ac ? "Mains power" : "Battery\(s.conditions.battery.map { " \($0)%" } ?? "")"
    out.append(Line(text: "\(power) · NAS \(s.conditions.nas ? "reachable" : "not reachable")", style: .small))
    out.append(Line(text: "\(s.host) · \(r.local ? "this Mac" : "via the NAS") · heard from \(duration(r.now - s.beat)) ago", style: .small))
    if let app = s.app { out.append(Line(text: "App \(app)", style: .small)) }
    if !s.waiting.isEmpty {
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Waiting", style: .header))
        for w in s.waiting.prefix(6) { out.append(Line(text: clip("\(w.what): \(w.why)", 90), style: .plain)) }
    }
    if !s.recent.isEmpty {
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Recent", style: .header))
        for d in s.recent.prefix(6) {
            let how = d.ok ? duration(d.secs) : "failed after \(duration(d.secs))"
            out.append(Line(text: clip("\(d.ok ? "✓" : "✗") \(d.what): \(how), \(clock(d.ended))", 90), style: .plain))
        }
    }
    if let built = s.built, !built.isEmpty {
        let b = built.values.reduce(0) { $0 + $1.built }, t = built.values.reduce(0) { $0 + $1.total }
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Areas built: \(b) of \(t) in \(built.count) region\(built.count == 1 ? "" : "s")", style: .small))
    }
    return out
}

/// What notifications compare: the job, whether it's paused, the last finished job, out of touch.
struct Seen {
    var job: String?
    var paused: Bool
    var lastEnded: Int
    var outOfTouch: Bool
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    lazy var item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    var reply: Reply?
    var seen: Seen?
    var polling = false
    /// Where notifications go: posted, or printed (--replay).
    var sink: (String, String) -> Void = { title, body in
        let c = UNMutableNotificationContent()
        c.title = title
        c.body = body
        c.threadIdentifier = "build"
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
    }

    func applicationDidFinishLaunching(_ n: Notification) {
        // Known to Launch Services by its bundle (notifications need it): the launcher starts the
        // executable inside it directly.
        LSRegisterURL(Bundle.main.bundleURL as CFURL, true)
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
        show()
        poll()
        Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in self?.poll() }
        Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { _ in quitIfReplaced() }
    }

    func poll() {
        if polling { return }
        polling = true
        var req = URLRequest(url: server.appendingPathComponent("api/build"))
        req.timeoutInterval = 4
        URLSession.shared.dataTask(with: req) { data, resp, _ in
            let ok = (resp as? HTTPURLResponse)?.statusCode == 200
            let r = ok ? data.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) } : nil
            DispatchQueue.main.async {
                self.polling = false
                self.reply = r
                self.notifyChanges()
                self.show()
            }
        }.resume()
    }

    // MARK: the icon and the menu

    func show() {
        let (kind, line) = classify(reply)
        if let b = item.button {
            let img = NSImage(systemSymbolName: kind.symbol, accessibilityDescription: line)
            img?.isTemplate = true
            b.image = img
            b.toolTip = line
        }
        item.menu = menu(kind, line)
    }

    func menu(_ kind: Kind, _ line: String) -> NSMenu {
        let m = NSMenu()
        m.autoenablesItems = false
        let size = NSFont.smallSystemFontSize
        for l in lines(reply, line) {
            if l.style == .separator {
                m.addItem(.separator())
                continue
            }
            let (font, color): (NSFont, NSColor) = switch l.style {
            case .title: (.boldSystemFont(ofSize: NSFont.systemFontSize), .labelColor)
            case .plain: (.menuFont(ofSize: 0), .labelColor)
            case .small, .header: (.menuFont(ofSize: size), .secondaryLabelColor)
            case .mono: (.monospacedSystemFont(ofSize: size - 1, weight: .regular), .secondaryLabelColor)
            case .separator: (.menuFont(ofSize: 0), .labelColor)
            }
            let it = NSMenuItem(title: l.text, action: nil, keyEquivalent: "")
            it.attributedTitle = NSAttributedString(string: l.text, attributes: [.font: font, .foregroundColor: color])
            m.addItem(it)
        }
        m.addItem(.separator())
        if let log = reply?.log {
            let it = NSMenuItem(title: "Open the Build Log", action: #selector(openLog), keyEquivalent: "")
            it.target = self
            it.representedObject = log
            m.addItem(it)
        }
        let map = NSMenuItem(title: "Open the Map", action: #selector(openMap), keyEquivalent: "")
        map.target = self
        m.addItem(map)
        return m
    }

    @objc func openLog(_ sender: NSMenuItem) {
        if let p = sender.representedObject as? String { NSWorkspace.shared.open(URL(fileURLWithPath: p)) }
    }

    @objc func openMap(_ sender: NSMenuItem) {
        NSWorkspace.shared.open(server.deletingLastPathComponent())
    }

    // MARK: notifications, for every change

    func notifyChanges() {
        guard let r = reply, let s = r.status else { return }
        let now = Seen(job: s.job?.id, paused: s.job?.paused != nil, lastEnded: s.recent.map(\.ended).max() ?? 0, outOfTouch: r.now - s.beat > outOfTouch)
        defer { seen = now }
        // The first answer only sets what changes are measured from.
        guard let was = seen else { return }
        if now.outOfTouch != was.outOfTouch {
            post(now.outOfTouch ? "Build Mac out of touch" : "Build Mac back", now.outOfTouch ? "Not heard from since \(clock(s.beat))" : s.host)
        }
        for d in s.recent.filter({ $0.ended > was.lastEnded }).sorted(by: { $0.ended < $1.ended }) {
            if d.ok {
                post("Done", "\(d.what) (\(duration(d.secs)))")
            } else {
                // The error's own line (the note is the exit status, then the log's last lines).
                let notes = (d.note ?? "").split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                let why = notes.last(where: { $0.hasPrefix("Error") }) ?? notes.last ?? ""
                post("Failed: \(d.what)", why.isEmpty ? "after \(duration(d.secs))" : clip(why, 160))
            }
        }
        if let j = s.job {
            if j.id != was.job {
                post("Building", j.what)
            } else if now.paused && !was.paused {
                post("Building paused", j.paused ?? j.what)
            } else if !now.paused && was.paused {
                post("Building resumed", j.what)
            }
        }
    }

    func post(_ title: String, _ body: String) {
        sink(title, body)
    }
}

/// Quits when the installed app's `current` link points at another version than this one's: the
/// launcher then starts the new one.
func quitIfReplaced() {
    let apps = home.appendingPathComponent("app").resolvingSymlinksInPath().path
    let mine = Bundle.main.bundleURL.resolvingSymlinksInPath().deletingLastPathComponent().path
    guard mine.hasPrefix(apps + "/") else { return }
    let current = home.appendingPathComponent("app/current").resolvingSymlinksInPath().path
    if current != mine { exit(0) }
}

let args = CommandLine.arguments
if args.contains("--print") {
    // The status now, as the icon and menu would show it.
    let done = DispatchSemaphore(value: 0)
    var r: Reply?
    URLSession.shared.dataTask(with: server.appendingPathComponent("api/build")) { data, _, _ in
        r = data.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) }
        done.signal()
    }.resume()
    done.wait()
    let (kind, line) = classify(r)
    print("icon: \(kind.symbol)")
    for l in lines(r, line) { print(l.style == .separator ? "────" : (l.style == .title ? "" : "  ") + l.text) }
} else if let i = args.firstIndex(of: "--replay") {
    // The notifications a sequence of answers would send.
    let d = AppDelegate()
    d.sink = { title, body in print("notify: \(title) — \(body)") }
    for f in args[(i + 1)...] {
        d.reply = try? JSONDecoder().decode(Reply.self, from: Data(contentsOf: URL(fileURLWithPath: f)))
        print("\(f): \(classify(d.reply).1)")
        d.notifyChanges()
    }
} else {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    app.run()
}
