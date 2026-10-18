// Scenic as an app (task #194): its build service, the map server's controls, quitting, and the
// app replacing itself on an update. The menu bar item and its panel are main.swift's.
//
// - The build service (this Mac's agent, `scenic agent`) is the app's own launchd job, registered
//   with SMAppService from the plist inside the app (Contents/Library/LaunchAgents/<label>.plist,
//   listed under Scenic in Login Items): launchd runs the app's launcher (Contents/MacOS/
//   scenic-launcher agent), which runs the installed app's agent from run/agent and starts it
//   again after a crash or for an update. Opening the app starts it; quitting the app asks it to
//   quit (crates/pipeline/src/control.rs Quit), and it exits cleanly, so launchd leaves it stopped.
// - The map server is a launchd service of its own (~/Library/LaunchAgents/local.scenic.server
//   .plist): the app shows whether it runs and starts or stops it when asked, never by itself.
// - An update: the server installs each published version under app/<version>; the app, installed
//   at /Applications/Scenic.app, copies the version's Scenic.app over itself and opens again (the
//   build service goes on, restarting into the new version by its own update drain).
//
// Info.plist names what the app manages, so a sandboxed copy (tools/status/sandbox.sh) manages its
// own and never the live services: ScenicBuildService (the job's label), ScenicServerLabel,
// ScenicHome and ScenicServer (else ~/Library/Application Support/scenic and
// http://127.0.0.1:7311), ScenicUpdateFrom (the bundle's name inside an installed version),
// ScenicOpenAtLogin (whether the app adds itself to the login items).

import AppKit
import ServiceManagement

let info = Bundle.main.infoDictionary ?? [:]
let buildLabel = info["ScenicBuildService"] as? String ?? "local.scenic.build"
let serverLabel = info["ScenicServerLabel"] as? String ?? "local.scenic.server"
let updateFrom = info["ScenicUpdateFrom"] as? String ?? "Scenic.app"
let openAtLogin = info["ScenicOpenAtLogin"] as? Bool ?? true
/// The agent's login item from before Scenic was an app: while it's there, the Mac hasn't switched
/// over (tools/app/install.sh), and the app's own build service is left alone.
let legacyAgent = info["ScenicLegacyAgent"] as? String ?? "local.scenic.agent"

/// Whether this copy of the app runs from inside an installed version (app/<version>/Scenic.app):
/// the menu bar item as the old login item (local.scenic.status) runs it, before the switch-over.
/// Such a copy manages nothing (no services registered, started or stopped, no window), and quits
/// when a newer version is installed, for that login item to start the new one.
func insideInstalledVersion() -> Bool {
    let apps = home.appendingPathComponent("app").resolvingSymlinksInPath().path
    return Bundle.main.bundleURL.resolvingSymlinksInPath().path.hasPrefix(apps + "/")
}

/// Why this app doesn't manage the build service now (nil: it does): run from inside an installed
/// version, or the Mac not switched over yet (the old agent's login item loaded or its plist there).
func notManaging() -> String? {
    if insideInstalledVersion() { return "run from inside an installed version: it manages nothing" }
    let plist = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/\(legacyAgent).plist")
    if jobState(legacyAgent) != .unloaded || FileManager.default.fileExists(atPath: plist.path) {
        return "Switch over with tools/app/install.sh: this Mac's agent still runs as \(legacyAgent)"
    }
    return nil
}

/// The copy run from inside an installed version (`insideInstalledVersion`), as the old login item
/// runs it: the version it started from, and its quit once `current` points elsewhere.
let launchedFrom = Bundle.main.bundleURL.resolvingSymlinksInPath().deletingLastPathComponent().path
func quitIfReplaced() {
    let current = home.appendingPathComponent("app/current").resolvingSymlinksInPath().path
    if current != launchedFrom {
        print("replaced by \(current)")
        exit(0)
    }
}

/// Runs a program and returns its exit status and output (stdout and stderr together).
@discardableResult
func run(_ path: String, _ args: [String]) -> (Int32, String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: path)
    p.arguments = args
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = pipe
    do { try p.run() } catch { return (-1, "\(error.localizedDescription)") }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(decoding: data, as: UTF8.self))
}

let domain = "gui/\(getuid())"

/// A launchd job as `launchctl print` says it: not loaded, loaded and stopped, or running (its pid).
enum JobState: Equatable {
    case unloaded
    case stopped
    case running(Int)

    var running: Bool { if case .running = self { return true } else { return false } }
}

func jobState(_ label: String) -> JobState {
    let (st, out) = run("/bin/launchctl", ["print", "\(domain)/\(label)"])
    if st != 0 { return .unloaded }
    let lines = out.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
    // (The job's own lines, before its nested sections: "state = running", "pid = 123".)
    if lines.contains("state = running"), let pid = lines.first(where: { $0.hasPrefix("pid = ") }).flatMap({ Int($0.dropFirst(6)) }) {
        return .running(pid)
    }
    return .stopped
}

// MARK: the build service

/// The app's build service: registered with SMAppService, started when the app opens.
final class BuildService {
    let service = SMAppService.agent(plistName: "\(buildLabel).plist")
    /// The quit this app asked of it, while it quits (`Finish`: the app waits for it).
    var quitting: String?
    var state: JobState = .unloaded
    var lastKick = Date.distantPast
    /// Why it's left alone (`notManaging`), if it is: nothing registered, started, tended or asked.
    var blocked: String?

    var agentDir: URL { home.appendingPathComponent("agent") }

    /// In words, for the window and the menu.
    var said: String {
        if let b = blocked { return "Build service: \(b)" }
        switch service.status {
        case .requiresApproval: return "Build service: off in Login Items (System Settings → General → Login Items)"
        case .notRegistered, .notFound: return "Build service: not set up"
        default: break
        }
        switch state {
        case .running(let pid): return quitting != nil ? "Build service: quitting (pid \(pid))" : "Build service: running (pid \(pid))"
        case .stopped, .unloaded: return "Build service: stopped"
        }
    }

    /// The app opened: a quit asked earlier (one a logout left) cancelled, the run file written,
    /// the job registered (launchd starts it) or started.
    func start() {
        blocked = notManaging()
        if blocked != nil { return }
        quitting = nil
        try? FileManager.default.removeItem(at: agentDir.appendingPathComponent("quit-request.json"))
        writeRunFile()
        switch service.status {
        case .enabled:
            break
        case .requiresApproval:
            SMAppService.openSystemSettingsLoginItems()
            return
        default:
            do { try service.register() } catch {
                NSLog("scenic: registering the build service: \(error)")
                if service.status == .requiresApproval { SMAppService.openSystemSettingsLoginItems() }
                return
            }
        }
        refresh()
        if !state.running { kick() }
    }

    func refresh() { state = jobState(buildLabel) }

    /// Started now (loaded by the registration: kickstart runs it).
    func kick() {
        lastKick = Date()
        run("/bin/launchctl", ["kickstart", "\(domain)/\(buildLabel)"])
        refresh()
    }

    /// Each poll: stopped while the app is open and not quitting (a quit a logout left, taken up
    /// before the app cleared it): started again, at most once a minute.
    func tend() {
        refresh()
        if blocked != nil { return }
        if quitting == nil, service.status == .enabled, !state.running, Date().timeIntervalSince(lastKick) > 60 {
            kick()
        }
    }

    /// Asks the agent to quit (control.rs Quit): `finish` lets its jobs end first.
    func askQuit(finish: Bool, why: String = "") {
        if blocked != nil { return }
        let name = ProcessInfo.processInfo.hostName.components(separatedBy: ".").first ?? "this Mac"
        let by = "Scenic on \(name)\(why)"
        let body: [String: Any] = ["mode": finish ? "finish" : "now", "by": by, "at": Int(Date().timeIntervalSince1970)]
        let (tmp, dst) = (agentDir.appendingPathComponent("quit-request.json.app.tmp"), agentDir.appendingPathComponent("quit-request.json"))
        do {
            try FileManager.default.createDirectory(at: agentDir, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: body).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            NSLog("scenic: asking the build service to quit: \(error)")
        }
        quitting = finish ? "finish" : "now"
    }

    /// A quit asked (`Finish this job first`) taken back: the build service goes on.
    func cancelQuit() {
        try? FileManager.default.removeItem(at: agentDir.appendingPathComponent("quit-request.json"))
        quitting = nil
    }

    /// The agent's run file for the app's launcher (run/agent): the installed app's agent, with the
    /// PATH its jobs need (osmium and Homebrew's tools, Java 21 for Planetiler, uv for the Python
    /// steps, wherever the login shell finds it). One that starts "# scenic: keep" is left alone (a
    /// sandbox's).
    func writeRunFile() {
        let runDir = info["ScenicRunDir"] as? String ?? home.appendingPathComponent("run").path
        let file = URL(fileURLWithPath: runDir).appendingPathComponent("agent")
        let old = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
        if old.hasPrefix("# scenic: keep") { return }
        let uv = run("/bin/zsh", ["-lc", "command -v uv"]).1.trimmingCharacters(in: .whitespacesAndNewlines)
        guard uv.hasPrefix("/") else {
            // (The file as it was, if any: the build service runs on it.)
            NSLog("scenic: uv not found: the agent's unit builds need it (install it with mise: see README)")
            return
        }
        let dir = (uv as NSString).deletingLastPathComponent
        let lines = ["/usr/bin/env", "PATH=\(dir):/opt/homebrew/opt/openjdk@21/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                     home.appendingPathComponent("app/current/scenic").path, "agent"]
        let new = lines.joined(separator: "\n") + "\n"
        if new != old {
            try? FileManager.default.createDirectory(atPath: runDir, withIntermediateDirectories: true)
            try? new.write(to: file, atomically: true, encoding: .utf8)
        }
    }
}

// MARK: the map server

/// The map server: its own launchd job, which the app starts and stops only when asked.
final class MapServer {
    var state: JobState = .unloaded
    var plist: URL { FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/\(serverLabel).plist") }
    var installed: Bool { FileManager.default.fileExists(atPath: plist.path) }

    func refresh() { state = jobState(serverLabel) }

    var said: String {
        if !installed { return "Map server: not installed (tools/app/install.sh)" }
        switch state {
        case .running(let pid): return "Map server: running at \(server.absoluteString) (pid \(pid))"
        case .stopped: return "Map server: stopped"
        case .unloaded: return "Map server: stopped (launchd starts it again at the next login, or start it here)"
        }
    }

    /// Started: loaded again if it was stopped (`stop` unloads it), else run now.
    func start() {
        if jobState(serverLabel) == .unloaded {
            run("/bin/launchctl", ["bootstrap", domain, plist.path])
        }
        run("/bin/launchctl", ["kickstart", "\(domain)/\(serverLabel)"])
        refresh()
    }

    /// Stopped: unloaded, as launchd would otherwise start it again (KeepAlive); it loads again at
    /// the next login, or when started here.
    func stop() {
        run("/bin/launchctl", ["bootout", "\(domain)/\(serverLabel)"])
        refresh()
    }
}

// MARK: the app's window

/// The window the app opens to (from the Dock, Launchpad or /Applications; not at login): the build
/// service's state, and the map server's with its controls.
final class StatusWindow: NSObject, NSWindowDelegate {
    let window: NSWindow
    let build = NSTextField(labelWithString: "")
    let map = NSTextField(labelWithString: "")
    let job = NSTextField(wrappingLabelWithString: "")
    let start = NSButton(title: "Start the Map Server", target: nil, action: nil)
    let stop = NSButton(title: "Stop the Map Server", target: nil, action: nil)
    let open = NSButton(title: "Open the Map", target: nil, action: nil)
    var onStart: () -> Void = {}
    var onStop: () -> Void = {}

    override init() {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 460, height: 190), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        super.init()
        window.title = Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "Scenic"
        window.isReleasedWhenClosed = false
        window.delegate = self
        for b in [start, stop, open] {
            b.target = self
            b.bezelStyle = .rounded
        }
        start.action = #selector(startServer)
        stop.action = #selector(stopServer)
        open.action = #selector(openMap)
        job.textColor = .secondaryLabelColor
        job.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        let buttons = NSStackView(views: [start, stop, open])
        buttons.spacing = 8
        let note = NSTextField(wrappingLabelWithString: "The map server is a service of its own: quitting Scenic leaves it as it is.")
        note.textColor = .secondaryLabelColor
        note.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        let stack = NSStackView(views: [build, job, NSBox.separator(), map, buttons, note])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 20, bottom: 16, right: 20)
        stack.translatesAutoresizingMaskIntoConstraints = false
        window.contentView = NSView()
        window.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: window.contentView!.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: window.contentView!.trailingAnchor),
            stack.topAnchor.constraint(equalTo: window.contentView!.topAnchor),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: window.contentView!.bottomAnchor),
            stack.widthAnchor.constraint(equalToConstant: 460),
        ])
        window.center()
    }

    func update(build b: BuildService, map m: MapServer, job j: String?) {
        build.stringValue = b.said
        job.stringValue = j ?? ""
        job.isHidden = j == nil
        map.stringValue = m.said
        start.isEnabled = m.installed && !m.state.running
        stop.isEnabled = m.state.running
        open.isEnabled = m.state.running
    }

    func show() {
        NSApp.activate()
        window.makeKeyAndOrderFront(nil)
    }

    @objc func startServer() { onStart() }
    @objc func stopServer() { onStop() }
    @objc func openMap() { NSWorkspace.shared.open(server.deletingLastPathComponent()) }
}

extension NSBox {
    static func separator() -> NSBox {
        let b = NSBox()
        b.boxType = .separator
        return b
    }
}

// MARK: this Mac's jobs, for quitting

/// A job this Mac's agent runs, as its own status file says (status.json, or helper.json for a
/// member): what it is, and its time left.
struct RunningJob {
    let what: String
    let step: String
    let left: Int?
    /// Its one target, when its id names one (`<step> <target>`): what a cancel names.
    var target: String? = nil
}

/// The fresh status file's jobs (its beat within the last six minutes; none from an agent stopped).
func runningJobs(serviceRunning: Bool) -> [RunningJob] {
    guard serviceRunning else { return [] }
    struct File: Decodable {
        let beat: Int
        let job: Job?
        let beside: Job?
        let more: [Job]?
    }
    let dir = home.appendingPathComponent("agent")
    let files = ["status.json", "helper.json"].compactMap { (try? Data(contentsOf: dir.appendingPathComponent($0))).flatMap { try? JSONDecoder().decode(File.self, from: $0) } }
    guard let f = files.max(by: { $0.beat < $1.beat }), Int(Date().timeIntervalSince1970) - f.beat < 6 * 60 else { return [] }
    return ([f.job, f.beside].compactMap { $0 } + (f.more ?? [])).map {
        let words = $0.id.split(separator: " ")
        return RunningJob(what: $0.what, step: String(words.first ?? ""), left: $0.progress?.eta_s, target: words.count == 2 ? String(words[1]) : nil)
    }
}

func jobsText(_ jobs: [RunningJob]) -> String {
    jobs.map { j in "\(j.what) (\(j.step))" + (j.left.map { ", about \(duration($0)) left" } ?? ", its time left unknown") }.joined(separator: "\n")
}

// MARK: the update

/// The version an app bundle was published as (Contents/Resources/version), if it says.
func bundleVersion(_ app: URL) -> String? {
    (try? String(contentsOf: app.appendingPathComponent("Contents/Resources/version"), encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines)
}

/// The installed version's app, when it's newer than this one (another version than this one's),
/// and this one is a copy of its own (in /Applications, not inside an installed version).
func newerApp() -> URL? {
    let me = Bundle.main.bundleURL.resolvingSymlinksInPath()
    if insideInstalledVersion() { return nil }
    let new = home.appendingPathComponent("app/current").resolvingSymlinksInPath().appendingPathComponent(updateFrom)
    guard let mine = bundleVersion(me), let theirs = bundleVersion(new), theirs != mine else { return nil }
    return new
}

/// Copies `new` over this app (beside it first, then swapped in by renames) and opens it once this
/// process has ended. False, and this app left as it was, if the copy fails.
func replaceSelf(with new: URL) -> Bool {
    let me = Bundle.main.bundleURL
    let dir = me.deletingLastPathComponent()
    let tmp = dir.appendingPathComponent(".\(me.lastPathComponent).new-\(getpid())")
    let old = dir.appendingPathComponent(".\(me.lastPathComponent).old-\(getpid())")
    try? FileManager.default.removeItem(at: tmp)
    let (st, out) = run("/usr/bin/ditto", [new.path, tmp.path])
    guard st == 0 else {
        NSLog("scenic: copying the new app: \(out)")
        try? FileManager.default.removeItem(at: tmp)
        return false
    }
    guard rename(me.path, old.path) == 0 else {
        try? FileManager.default.removeItem(at: tmp)
        return false
    }
    guard rename(tmp.path, me.path) == 0 else {
        _ = rename(old.path, me.path)
        try? FileManager.default.removeItem(at: tmp)
        return false
    }
    try? FileManager.default.removeItem(at: old)
    // (Opened again once this process is gone, `open` would bring this one forward; without its window.)
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/bin/sh")
    p.arguments = ["-c", "while kill -0 \(getpid()) 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$0\" --args --no-window", me.path]
    try? p.run()
    return true
}

/// Why the system quits the app, if it's for a logout, restart or shutdown.
func quitForLogout() -> Bool {
    guard let e = NSAppleEventManager.shared().currentAppleEvent,
          let why = e.attributeDescriptor(forKeyword: AEKeyword(kAEQuitReason))?.typeCodeValue else { return false }
    return [kAELogOut, kAEReallyLogOut, kAEShowRestartDialog, kAEShowShutdownDialog, kAERestart, kAEShutDown].map { OSType($0) }.contains(why)
}

/// Whether the app was opened as a login item (no window then).
func launchedAtLogin() -> Bool {
    guard let e = NSAppleEventManager.shared().currentAppleEvent else { return false }
    return e.eventID == kAEOpenApplication && e.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
}
