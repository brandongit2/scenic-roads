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
//   scenic-status --render menu.png          the menu's lines drawn as they lay out (dark), for checking
//   scenic-status --wait-replaced           waits, without a window, until a newer app is installed
// SCENIC_STATUS_SERVER overrides the server (http://127.0.0.1:8080), SCENIC_HOME the app folder.

import AppKit
import CoreServices
import UserNotifications

let server = URL(string: ProcessInfo.processInfo.environment["SCENIC_STATUS_SERVER"] ?? "http://127.0.0.1:8080")!
let home = ProcessInfo.processInfo.environment["SCENIC_HOME"].map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/scenic")
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
    /// The build to the end (agents from 2026-10-03 on).
    let checklist: [Step]?
    /// Other Macs helping, building units (agents from 2026-10-04 on).
    let helpers: [Helper]?
    /// Every worker the coordinator heard from lately: helpers and web pages (agents from 2026-10-05 on).
    let workers: [Worker]?
}

/// A worker, as the build Mac's coordinator knows it (docs/workers.md).
struct Worker: Decodable {
    let name: String
    let label: String
    let kind: String
    let what: String
    let done: Int
    let bad: Bool
}

/// A helper on another Mac, as the main agent last read its status.
struct Helper: Decodable {
    let host: String
    let beat: Int
    let job: Job?
}

/// A step of the build to the end: done of total (total unknown until an earlier step makes it), or
/// for a group of single jobs how many are left.
struct Step: Decodable {
    let what: String
    let steps: [String]
    let done: Int?
    let total: Int?
    let left: Int?
    let unit: String?

    var finished: Bool {
        if let l = left { return l == 0 }
        if let t = total, let d = done { return d >= t }
        return false
    }
}

struct JobProgress: Decodable {
    let done: Double
    let total: Double
    let unit: String
    let eta_s: Int?
}

struct Conditions: Decodable {
    let ac: Bool
    let nas: Bool
    /// At home (false: the NAS through Tailscale); absent from older heartbeats.
    let home: Bool?
    let battery: Int?
}

struct Job: Decodable {
    let id: String
    let what: String
    let started: Int
    let paused: String?
    let tail: String?
    let progress: JobProgress?
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

/// "45 s", "12 min", "3 h 5 min", "2 days" (number and unit never split across lines).
func duration(_ secs: Int) -> String {
    let nb = "\u{00A0}"
    if secs < 60 { return "\(max(secs, 0))\(nb)s" }
    if secs < 3600 { return "\(secs / 60)\(nb)min" }
    if secs < 2 * 86400 {
        let m = secs / 60 % 60
        return m == 0 ? "\(secs / 3600)\(nb)h" : "\(secs / 3600)\(nb)h\(nb)\(m)\(nb)min"
    }
    return "\(secs / 86400)\(nb)days"
}

func clock(_ t: Int) -> String {
    let f = DateFormatter()
    f.dateFormat = Calendar.current.isDateInToday(Date(timeIntervalSince1970: TimeInterval(t))) ? "HH:mm" : "d MMM HH:mm"
    return f.string(from: Date(timeIntervalSince1970: TimeInterval(t)))
}

/// Home folders as "~" (the build Mac's paths, as its log shows them).
func tildes(_ s: String) -> String {
    s.replacingOccurrences(of: "/Users/[^/ ]+/", with: "~/", options: .regularExpression)
}

func clip(_ s: String, _ n: Int = 80) -> String {
    s.count <= n ? s : String(s.prefix(n - 1)) + "…"
}

/// The helpers heard from lately (one not heard from for `outOfTouch` is left out).
func freshHelpers(_ r: Reply) -> [Helper] {
    (r.status?.helpers ?? []).filter { r.now - $0.beat <= outOfTouch }
}

/// The state, with a line saying it.
func classify(_ r: Reply?) -> (Kind, String) {
    guard let r = r else { return (.unknown, "The map's server on this Mac isn't answering") }
    guard let s = r.status else { return (.unknown, "No word from the build Mac (is the NAS reachable?)") }
    // A helper building counts as building, whatever the build Mac is doing.
    let helping = freshHelpers(r).filter { $0.job != nil && $0.job?.paused == nil }
    if r.now - s.beat > outOfTouch {
        if let h = helping.first { return (.building, clip("Building on \(h.host); build Mac out of touch since \(clock(s.beat))")) }
        return (.outOfTouch, "Build Mac out of touch since \(clock(s.beat))")
    }
    if let j = s.job {
        if j.paused != nil {
            if let h = helping.first { return (.building, clip("Building on \(h.host); the build Mac's job paused")) }
            return (.paused, "Paused")
        }
        return (.building, "Building")
    }
    if let h = helping.first { return (.building, clip("Building on \(h.host)")) }
    if let w = s.waiting.first(where: { $0.why.contains("failed") }) { return (.problem, clip("Failed: \(w.what)")) }
    if let d = s.recent.first, !d.ok { return (.problem, clip("Failed: \(d.what)")) }
    if let w = s.waiting.first { return (.waiting, clip("Waiting: \(w.what)")) }
    return (.idle, "Nothing to build")
}

/// One line of the menu: plain, small and dim, or the log's monospace.
enum Style {
    case title, plain, small, mono, header, separator, bar, stepDone, stepNow, stepToDo
}

struct Line {
    let text: String
    let style: Style
    /// For a bar: how far (0–1).
    var fraction: Double = 0
}

/// "12,345".
func grouped(_ v: Double) -> String {
    let f = NumberFormatter()
    f.numberStyle = .decimal
    f.maximumFractionDigits = 0
    return f.string(from: NSNumber(value: v)) ?? "\(Int(v))"
}

/// The menu's lines for an answer, under the state's line.
func lines(_ r: Reply?, _ line: String) -> [Line] {
    var out = [Line(text: line, style: .title)]
    guard let r = r, let s = r.status else { return out }
    if let j = s.job {
        out.append(Line(text: j.what, style: .plain))
        // How far the job says it is, and the time it has left at its pace.
        if let p = j.progress {
            let frac = p.total > 0 ? p.done / p.total : 0
            var t = "\(Int((frac * 100).rounded(.down)))% · \(grouped(p.done)) of \(grouped(p.total)) \(p.unit)"
            if let e = p.eta_s, j.paused == nil { t += " · about \(duration(e)) left" }
            out.append(Line(text: t, style: .bar, fraction: frac))
        }
        if let p = j.paused { out.append(Line(text: p, style: .small)) }
        out.append(Line(text: "Running \(duration(r.now - j.started)) (since \(clock(j.started)))", style: .small))
        // The log's last lines, without the terminal's colour codes.
        let plain = (j.tail ?? "").replacingOccurrences(of: "\u{1B}\\[[0-9;]*[A-Za-z]", with: "", options: .regularExpression)
        for l in plain.split(separator: "\n").suffix(3) where !l.trimmingCharacters(in: .whitespaces).isEmpty {
            out.append(Line(text: tildes(String(l).trimmingCharacters(in: .whitespaces)), style: .mono))
        }
    }
    // The other Macs helping (units only), each with its job.
    for h in freshHelpers(r) {
        guard let j = h.job else {
            out.append(Line(text: "\(h.host) is helping; nothing for it to build now", style: .small))
            continue
        }
        out.append(Line(text: "\(h.host): \(j.what)", style: .plain))
        if let p = j.progress {
            let frac = p.total > 0 ? p.done / p.total : 0
            var t = "\(Int((frac * 100).rounded(.down)))% · \(grouped(p.done)) of \(grouped(p.total)) \(p.unit)"
            if let e = p.eta_s, j.paused == nil { t += " · about \(duration(e)) left" }
            out.append(Line(text: t, style: .bar, fraction: frac))
        }
        if let p = j.paused { out.append(Line(text: "\(h.host): \(p)", style: .small)) }
    }
    // Web pages working for the build (the helpers are above).
    for w in (s.workers ?? []) where w.kind == "web" {
        let done = w.done > 0 ? " · \(w.done) done" : ""
        out.append(Line(text: w.bad ? "\(w.label): stopped (a result differed)" : "\(w.label): \(clip(w.what, 60))\(done)", style: .small))
    }
    let power = s.conditions.ac ? "Mains power" : "Battery\(s.conditions.battery.map { " \($0)%" } ?? "")"
    out.append(Line(text: "\(power) · NAS \(!s.conditions.nas ? "not reachable" : s.conditions.home == false ? "through Tailscale" : "reachable")", style: .small))
    out.append(Line(text: "\(s.host) · \(r.local ? "this Mac" : "via the NAS") · heard from \(duration(r.now - s.beat)) ago", style: .small))
    if let app = s.app { out.append(Line(text: "App \(app)", style: .small)) }
    // The build to the end: each step done, under way, or to come.
    if let steps = s.checklist, !steps.isEmpty {
        let now = s.job.map { String($0.id.split(separator: " ").first ?? "") }
        let finished = steps.filter(\.finished).count
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "To the end: \(finished) of \(steps.count) steps done", style: .header))
        for st in steps {
            var count = ""
            if let t = st.total, let d = st.done, !st.finished {
                count = ": \(grouped(Double(d))) of \(grouped(Double(t))) \(st.unit ?? "")"
            } else if let l = st.left, l > 0 {
                count = l == 1 ? ": 1 job left" : ": \(l) jobs left"
            }
            if st.finished {
                out.append(Line(text: "✓ \(st.what)", style: .stepDone))
            } else if let n = now, st.steps.contains(n) {
                out.append(Line(text: "▸ \(st.what)\(count)", style: .stepNow))
            } else {
                out.append(Line(text: "○ \(st.what)\(count)", style: .stepToDo))
            }
        }
    }
    if !s.waiting.isEmpty {
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Waiting", style: .header))
        for w in s.waiting.prefix(6) { out.append(Line(text: "\(w.what): \(w.why)", style: .plain)) }
    }
    if !s.recent.isEmpty {
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Recent", style: .header))
        // The last jobs, a run of the same job with the same outcome as one entry.
        var runs: [(Done, Int)] = []
        for d in s.recent {
            if let last = runs.last, last.0.id == d.id, last.0.ok == d.ok {
                runs[runs.count - 1].1 += 1
            } else {
                runs.append((d, 1))
            }
        }
        for (d, n) in runs.prefix(6) {
            out.append(Line(text: "\(d.ok ? "✓" : "✗") \(d.what)", style: .plain))
            let times = n > 1 ? "\(d.ok ? "done" : "failed") \(n) times, the last" : (d.ok ? "done" : "failed")
            let how = d.ok ? "in \(duration(d.secs))" : "after \(duration(d.secs))"
            out.append(Line(text: "\(times) \(how) at \(clock(d.ended))", style: .small))
        }
    }
    if let built = s.built, !built.isEmpty {
        // Whole regions (the checklist counts the areas, each once: regions share areas).
        let done = built.values.filter { $0.total > 0 && $0.built >= $0.total }.count
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Regions built: \(done) of \(built.count)", style: .small))
    }
    return out
}

/// A line's font and colour in the menu.
func fontFor(_ style: Style) -> (NSFont, NSColor) {
    let size = NSFont.smallSystemFontSize
    switch style {
    case .title: return (.boldSystemFont(ofSize: NSFont.systemFontSize), .labelColor)
    case .plain, .separator, .stepNow: return (.menuFont(ofSize: 0), .labelColor)
    case .small, .header, .bar, .stepDone: return (.menuFont(ofSize: size), .secondaryLabelColor)
    case .stepToDo: return (.menuFont(ofSize: 0), .secondaryLabelColor)
    case .mono: return (.monospacedSystemFont(ofSize: size - 1, weight: .regular), .secondaryLabelColor)
    }
}

/// A progress bar with its line of text under it (the menu's, not clickable).
final class BarView: NSView {
    init(_ text: String, fraction: Double) {
        let w = LineView.width - 2 * LineView.inset
        let (font, color) = fontFor(.bar)
        let label = LineView(text, font: font, color: color, wrapAnywhere: false)
        let barH: CGFloat = 12
        super.init(frame: NSRect(x: 0, y: 0, width: LineView.width, height: label.frame.height + barH + 4))
        let bar = NSProgressIndicator(frame: NSRect(x: LineView.inset, y: label.frame.height + 2, width: w, height: barH))
        bar.style = .bar
        bar.isIndeterminate = false
        bar.controlSize = .small
        bar.minValue = 0
        bar.maxValue = 1
        bar.doubleValue = max(0, min(1, fraction))
        addSubview(bar)
        label.setFrameOrigin(.zero)
        addSubview(label)
    }

    required init?(coder: NSCoder) {
        fatalError("not from a nib")
    }
}

/// A menu line that wraps within the menu's width instead of being cut short with "…", and isn't
/// clickable (a view: no highlight, no action).
final class LineView: NSView {
    static let width: CGFloat = 420
    /// The menu's text inset, as its own items have it.
    static let inset: CGFloat = 14

    init(_ text: String, font: NSFont, color: NSColor, wrapAnywhere: Bool) {
        let w = LineView.width - 2 * LineView.inset
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = font
        label.textColor = color
        label.isSelectable = false
        label.lineBreakMode = wrapAnywhere ? .byCharWrapping : .byWordWrapping
        label.preferredMaxLayoutWidth = w
        let h = ceil(label.cell?.cellSize(forBounds: NSRect(x: 0, y: 0, width: w, height: 10_000)).height ?? font.pointSize + 4)
        super.init(frame: NSRect(x: 0, y: 0, width: LineView.width, height: h + 4))
        label.frame = NSRect(x: LineView.inset, y: 2, width: w, height: h)
        addSubview(label)
    }

    required init?(coder: NSCoder) {
        fatalError("not from a nib")
    }
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
        _ = launchedFrom
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
        for l in lines(reply, line) {
            if l.style == .separator {
                m.addItem(.separator())
                continue
            }
            let (font, color) = fontFor(l.style)
            // A line of information, not a button: it wraps rather than being cut short, and
            // neither highlights nor does anything when clicked.
            let it = NSMenuItem(title: l.text, action: nil, keyEquivalent: "")
            it.view = l.style == .bar ? BarView(l.text, fraction: l.fraction) : LineView(l.text, font: font, color: color, wrapAnywhere: l.style == .mono)
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
        // The worker page's address, with its token (the build Mac's agent writes it, private to this
        // user): pasted on another device (Universal Clipboard), its browser joins the build.
        if let page = try? String(contentsOf: home.appendingPathComponent("agent/coord/page"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), !page.isEmpty {
            let it = NSMenuItem(title: "Copy the Worker Page's Address", action: #selector(copyPage), keyEquivalent: "")
            it.target = self
            it.representedObject = page
            m.addItem(it)
        }
        return m
    }

    @objc func copyPage(_ sender: NSMenuItem) {
        guard let page = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(page, forType: .string)
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

/// The app version folder this process was started from, resolved when it starts (it's run through
/// the `current` link, which later points elsewhere).
let launchedFrom = Bundle.main.bundleURL.resolvingSymlinksInPath().deletingLastPathComponent().path

/// Quits when the installed app's `current` link points at another version than the one this
/// process started from: the launcher then starts the new one.
func quitIfReplaced() {
    let apps = home.appendingPathComponent("app").resolvingSymlinksInPath().path
    guard launchedFrom.hasPrefix(apps + "/") else { return }
    let current = home.appendingPathComponent("app/current").resolvingSymlinksInPath().path
    if current != launchedFrom {
        print("replaced by \(current)")
        exit(0)
    }
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
    for l in lines(r, line) {
        let bar = l.style == .bar ? "[" + String(repeating: "█", count: Int(l.fraction * 20)) + String(repeating: "░", count: 20 - Int(l.fraction * 20)) + "] " : ""
        print(l.style == .separator ? "────" : (l.style == .title ? "" : "  ") + bar + l.text)
    }
} else if let i = args.firstIndex(of: "--render"), i + 1 < args.count {
    // The menu's information lines as views, stacked as the menu stacks them, drawn into a PNG.
    let done = DispatchSemaphore(value: 0)
    var r: Reply?
    URLSession.shared.dataTask(with: server.appendingPathComponent("api/build")) { data, _, _ in
        r = data.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) }
        done.signal()
    }.resume()
    done.wait()
    _ = NSApplication.shared
    let d = AppDelegate()
    d.reply = r
    let (kind, line) = classify(r)
    let views: [NSView] = lines(r, line).map { l in
        l.style == .separator ? NSView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: 11)) : l.style == .bar ? BarView(l.text, fraction: l.fraction) : LineView(l.text, font: fontFor(l.style).0, color: fontFor(l.style).1, wrapAnywhere: l.style == .mono)
    } + (r?.log != nil ? ["Open the Build Log"] : []).map { LineView($0, font: .menuFont(ofSize: 0), color: .labelColor, wrapAnywhere: false) }
        + [LineView("Open the Map", font: .menuFont(ofSize: 0), color: .labelColor, wrapAnywhere: false)]
    _ = (d, kind)
    let height = views.reduce(CGFloat(0)) { $0 + $1.frame.height } + 16
    let canvas = NSView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: height))
    canvas.appearance = NSAppearance(named: .darkAqua)
    canvas.wantsLayer = true
    canvas.layer?.backgroundColor = NSColor(white: 0.17, alpha: 1).cgColor
    var y = height - 8
    for v in views {
        y -= v.frame.height
        v.setFrameOrigin(NSPoint(x: 0, y: y))
        canvas.addSubview(v)
    }
    let rep = canvas.bitmapImageRepForCachingDisplay(in: canvas.bounds)!
    NSAppearance(named: .darkAqua)!.performAsCurrentDrawingAppearance { canvas.cacheDisplay(in: canvas.bounds, to: rep) }
    try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: args[i + 1]))
    print("drew \(views.count) lines, \(Int(height)) pt high")
} else if args.contains("--wait-replaced") {
    // What the menu bar item does every minute, every second: quits once replaced.
    _ = launchedFrom
    print("started from \(launchedFrom)")
    while true {
        quitIfReplaced()
        Thread.sleep(forTimeInterval: 1)
    }
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
