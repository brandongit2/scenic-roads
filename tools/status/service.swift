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
//   at /Applications/Scenic.app, copies the version's Scenic.app over itself and quits, and a
//   helper (`--reopen`) opens the new one once it has ended, retrying until it runs (the build
//   service's agent goes on, restarting into the new version by its own update drain). The
//   launcher launchd started would go on from the old bundle, renamed aside, without the app's
//   NAS permission (task #389): the app opened again finds it running from outside itself, asks
//   it to stop and starts the job once it has ended (`followApp`), then deletes the old bundle;
//   each poll checks again.
//
// Info.plist names what the app manages, so a sandboxed copy (tools/status/sandbox.sh) manages its
// own and never the live services: ScenicBuildService (the job's label), ScenicServerLabel,
// ScenicHome and ScenicServer (else ~/Library/Application Support/scenic and
// http://127.0.0.1:7311), ScenicUpdateFrom (the bundle's name inside an installed version),
// ScenicOpenAtLogin (whether the app adds itself to the login items).

import AppKit
import Security
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

func jobState(_ label: String) -> JobState { launchdPrint(label).map(parseJobState) ?? .unloaded }

/// `launchctl print`'s output for a job of this user's domain, nil when it isn't loaded.
func launchdPrint(_ label: String) -> String? {
    let (st, out) = run("/bin/launchctl", ["print", "\(domain)/\(label)"])
    return st == 0 ? out : nil
}

/// The job's own lines (one tab deep in `launchctl print`, not its nested sections' such as a
/// coalition's "state = active"), trimmed.
func jobLines(_ out: String) -> [String] {
    out.split(separator: "\n").filter { $0.hasPrefix("\t") && !$0.hasPrefix("\t\t") }.map { $0.trimmingCharacters(in: .whitespaces) }
}

func parseJobState(_ out: String) -> JobState {
    let lines = jobLines(out)
    if lines.contains("state = running"), let pid = lines.first(where: { $0.hasPrefix("pid = ") }).flatMap({ Int($0.dropFirst(6)) }) {
        return .running(pid)
    }
    return .stopped
}

/// Why launchd can't start the job, from its `launchctl print` (nil: nothing says so). A launcher
/// whose code signature changed (a new cdhash) no longer matches the launch constraint (LWCR)
/// launchd kept from the registration: "needs LWCR update" among its properties, and each spawn
/// fails with EX_CONFIG (state "spawn failed", last exit code 78). Registering it again clears it.
func spawnBroken(_ out: String) -> String? {
    let lines = jobLines(out)
    func value(_ key: String) -> String { lines.first { $0.hasPrefix(key + " = ") }.map { String($0.dropFirst(key.count + 3)) } ?? "" }
    if value("properties").contains("needs LWCR update") { return "its launch constraint is out of date (needs LWCR update)" }
    if (value("state") == "spawn failed" || value("job state") == "spawn failed") && value("last exit code").hasPrefix("78") {
        return "spawn failed, last exit code \(value("last exit code"))"
    }
    return nil
}

/// The cdhash of a program's code signature, in hex (nil: unsigned or unreadable).
func cdhash(_ path: String) -> String? {
    var code: SecStaticCode?
    guard SecStaticCodeCreateWithPath(URL(fileURLWithPath: path) as CFURL, [], &code) == errSecSuccess, let code else { return nil }
    var info: CFDictionary?
    guard SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
          let d = (info as? [String: Any])?[kSecCodeInfoUnique as String] as? Data else { return nil }
    return d.map { String(format: "%02x", $0) }.joined()
}

/// The app's launcher (the build service's launchd program) in a bundle.
func launcherPath(_ bundle: URL) -> String { bundle.appendingPathComponent("Contents/MacOS/scenic-launcher").path }

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
        if let b = broken { return "Build service: launchd can't start it: \(b)" }
        switch service.status {
        case .requiresApproval: return "Build service: off in Login Items (System Settings → General → Login Items)"
        case .notRegistered, .notFound: return "Build service: not set up"
        default: break
        }
        switch state {
        case .running(let pid) where refused && quitting == nil: return "Build service: running (pid \(pid)), but it can't reach the NAS: allow Scenic in Files and Folders"
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
            // (Registered by an app whose launcher was signed otherwise, as after an update that
            // changed it: launchd's launch constraint names the old cdhash and refuses the new one.)
            if let was = UserDefaults.standard.string(forKey: launcherKey), let now = cdhash(launcherPath(Bundle.main.bundleURL)), was != now {
                reregister("the launcher's signature changed (cdhash \(was.prefix(12))… → \(now.prefix(12))…)")
            }
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
        noteLauncher()
        refresh()
        repair()
        if !state.running { kick() }
        followApp()
    }

    func refresh() {
        let out = launchdPrint(buildLabel)
        state = out.map(parseJobState) ?? .unloaded
        broken = out.flatMap(spawnBroken)
    }
    /// Why launchd can't start it (`spawnBroken`), as the last refresh found.
    var broken: String?

    /// The cdhash of the launcher the service was registered with (kept in the app's defaults:
    /// the sandbox's are its own), noted while it's registered.
    let launcherKey = "registeredLauncherCDHash"
    func noteLauncher() {
        if service.status == .enabled, let h = cdhash(launcherPath(Bundle.main.bundleURL)) { UserDefaults.standard.set(h, forKey: launcherKey) }
    }

    /// launchd refuses to start it (`broken`): registered again and started. At launch and each poll.
    func repair() {
        guard blocked == nil, quitting == nil, let why = broken else { return }
        reregister("launchd can't start the build service: \(why)")
    }

    /// The job registered afresh from this app (unregistered, gone from launchd, registered: a new
    /// launch constraint) and started, each step in update.log. Registering right after an update
    /// swapped the app can keep the old constraint (the M4, 23 Oct: "needs LWCR update" again; 30 s
    /// later it took), so it's tried again while the job is refused, 30 s apart, 4 times a launch at
    /// most: after that it's logged and left (by hand: `scenic-status --unregister`, quit, open the app).
    func reregister(_ why: String) {
        guard reregistrations < 4, Date().timeIntervalSince(lastReregister) > 30 else {
            if reregistrations >= 4, loggedLeft != why { updateLog("\(why); registered again 4 times this launch already: left as it is"); loggedLeft = why }
            return
        }
        reregistrations += 1
        lastReregister = Date()
        updateLog("\(why): registering the build service again (\(reregistrations) of 4 this launch)")
        do { try service.unregister() } catch { updateLog("unregistering the build service: \(error)") }
        for _ in 0..<25 {
            if jobState(buildLabel) == .unloaded { break }
            Thread.sleep(forTimeInterval: 0.2)
        }
        do { try service.register() } catch { updateLog("registering the build service: \(error)") }
        noteLauncher()
        refresh()
        if !state.running { kick() }
        updateLog("the build service registered again: \(broken.map { "still \($0)" } ?? (state.running ? "running" : "not running"))")
        endStaleLaunchers()
    }

    /// Launchers of this app (or of a copy an update left aside) other than the job's own, ended
    /// (SIGTERM to each exact pid: it passes it on to its agent, which leaves its jobs for the next,
    /// and exits). Unregistering doesn't always end the job's process (the M1, 22 Oct): left, its
    /// agent holds the lock under no registered job. launchd's state read afresh first, and only
    /// launchers that started before the job's own (a launcher launchd has just started is never
    /// one). The agent ends such a holder too (crates/pipeline/src/agent/takeover.rs); this ends
    /// the launcher, which would start it again. Each pid logged once.
    func endStaleLaunchers() {
        refresh()
        guard case .running(let job) = state, let jobStart = processStart(Int32(job)) else { return }
        let found = launcherProcesses().compactMap { l in processStart(l.0).map { (l.0, $0, l.1) } }
        for pid in staleLaunchers(found.map { ($0.0, $0.1) }, job: (Int32(job), jobStart)) where kill(pid, SIGTERM) == 0 {
            if endedLaunchers.insert(pid).inserted {
                updateLog("ended a stale launcher (pid \(pid), \(found.first { $0.0 == pid }?.2 ?? "?")): not the build service's (pid \(job))")
            }
        }
    }
    /// The launchers `endStaleLaunchers` ended (logged once each).
    var endedLaunchers = Set<Int32>()

    /// The running launchers of this app or of its copies left aside (.<name>.old-<pid>): pid and
    /// executable.
    func launcherProcesses() -> [(Int32, String)] {
        let me = Bundle.main.bundleURL
        guard let dir = realPath(me.deletingLastPathComponent().path) else { return [] }
        let mine = launcherPath(URL(fileURLWithPath: dir).appendingPathComponent(me.lastPathComponent))
        let aside = "\(dir)/.\(me.lastPathComponent).old-"
        return allPids().compactMap { pid in
            guard let p = executablePath(pid).map({ realPath($0) ?? $0 }) else { return nil }
            let ours = p == mine || (p.hasPrefix(aside) && p.hasSuffix("/Contents/MacOS/scenic-launcher"))
            return ours ? (pid, p) : nil
        }
    }
    /// Times registered again this launch (`reregister`), and when last.
    var reregistrations = 0
    var lastReregister = Date.distantPast
    var loggedLeft: String?

    /// The running launcher's executable when it isn't this app's (nil: it is, or nothing runs).
    /// After an update swapped the app, the launcher launchd started goes on from the old bundle,
    /// renamed aside (.Scenic.app.old-<pid>); macOS's network-volume permission is the app's at its
    /// own path, so the agent it runs is refused the NAS (EPERM, task #389).
    func launcherElsewhere() -> String? {
        guard case .running(let pid) = state else { return nil }
        // (Unreadable: its executable deleted, as an older app's update did: elsewhere too.)
        guard let path = executablePath(Int32(pid)) else { return "pid \(pid), its executable unreadable" }
        guard let mine = realPath(Bundle.main.bundleURL.path) else { return nil }
        return (realPath(path) ?? path).hasPrefix(mine + "/") ? nil : path
    }

    /// The launcher restarted from this app when it runs from elsewhere (`launcherElsewhere`), a
    /// step at launch and each poll (`followStep`): asked to stop (`launchctl kill SIGTERM`, at most
    /// once a minute), which it passes on to its agent and waits for; once that launcher has ended,
    /// the job started (launchd runs it from the registered bundle, this one at the same path);
    /// still elsewhere after that, the job registered afresh (unregistered and registered) and
    /// started. Never `kickstart -k`: launchd SIGKILLs a job that hasn't exited ~5 s after its
    /// SIGTERM, and an agent may take minutes to stop (a loop phase), so the launcher was killed and
    /// its agent left an orphan (#799). The copies an update left aside deleted once nothing runs
    /// from them.
    func followApp() {
        defer { removeOldCopies() }
        guard blocked == nil, quitting == nil else { return }
        let ended = asked.map { processStart($0.0) != $0.1 } ?? false
        switch followStep(elsewhere: launcherElsewhere() != nil, asked: asked != nil, ended: ended,
                          sinceAsk: Date().timeIntervalSince(lastFollow)) {
        case .wait: return
        case .stop:
            guard case .running(let pid) = state, let start = processStart(Int32(pid)) else { return }
            lastFollow = Date()
            asked = (Int32(pid), start)
            updateLog("the build service's launcher (pid \(pid)) runs from \(launcherElsewhere() ?? "?"), not this app: asking it to stop (SIGTERM), then starting it again")
            run("/bin/launchctl", ["kill", "SIGTERM", "\(domain)/\(buildLabel)"])
        case .start:
            asked = nil
            kick()
            if let still = launcherElsewhere() { reregister("the launcher still runs from \(still)") }
        }
    }
    var lastFollow = Date.distantPast
    /// The launcher `followApp` asked to stop (pid, start), until it has ended.
    var asked: (Int32, UInt64)?

    /// The app's copies left aside by updates (.<name>.old-<pid> beside it), deleted once no process
    /// runs from them.
    func removeOldCopies() {
        let me = Bundle.main.bundleURL
        let dir = me.deletingLastPathComponent()
        let prefix = ".\(me.lastPathComponent).old-"
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: dir.path) else { return }
        let real = realPath(dir.path) ?? dir.path
        let olds = names.filter { $0.hasPrefix(prefix) }.map { real + "/" + $0 + "/" }
        if olds.isEmpty { return }
        let used = runningExecutables().map { realPath($0) ?? $0 }
        for o in olds where !used.contains(where: { $0.hasPrefix(o) }) {
            do { try FileManager.default.removeItem(atPath: o) } catch { NSLog("scenic: removing \(o): \(error)") }
        }
    }

    /// The agent refused the NAS since it last started: in its log (~/Library/Logs/scenic/agent.log),
    /// "Operation not permitted" on a /Volumes/ path after the last "agent: started" in its last
    /// 256 KB (no such line there: false). Read again only when the log's size or mtime changed.
    func nasRefused() -> Bool {
        guard state.running else { return false }
        guard let a = try? FileManager.default.attributesOfItem(atPath: agentLog) else { return false }
        let stamp = "\((a[.size] as? NSNumber)?.int64Value ?? -1) \((a[.modificationDate] as? Date)?.timeIntervalSince1970 ?? -1)"
        if stamp == logStamp { return refused }
        logStamp = stamp
        guard let h = FileHandle(forReadingAtPath: agentLog) else { return false }
        defer { try? h.close() }
        let end = (try? h.seekToEnd()) ?? 0
        try? h.seek(toOffset: end > 262_144 ? end - 262_144 : 0)
        let text = String(decoding: (try? h.readToEnd()) ?? Data(), as: UTF8.self)
        guard let r = text.range(of: "agent: started", options: .backwards) else { return false }
        return text[r.upperBound...].split(separator: "\n").contains { $0.contains("Operation not permitted") && $0.contains("/Volumes/") }
    }
    var logStamp = ""
    let agentLog = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/scenic/agent.log").path
    /// The agent refused the NAS (`nasRefused`), as the last poll found.
    var refused = false
    static let refusedLine = "This Mac's build service can't reach the NAS: allow Scenic in System Settings → Privacy & Security → Files and Folders"

    /// Left alone, asked again (each poll, and as the menu opens): once the switch-over is done
    /// (the old login items removed after the app opened), started as at launch. True when it was
    /// started now.
    func recheck() -> Bool {
        guard blocked != nil, notManaging() == nil else { return false }
        start()
        return blocked == nil
    }

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
        repair()
        if quitting == nil, service.status == .enabled, !state.running, Date().timeIntervalSince(lastKick) > 60 {
            kick()
        }
        followApp()
        endStaleLaunchers()
        refused = nasRefused()
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

/// The window the app opens to (from Launchpad, Spotlight or /Applications, or again while it runs; not at login): the build
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
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 460, height: 190), styleMask: [.titled, .closable], backing: .buffered, defer: false)
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
        /// Its jobs (agents from 2026-10-18 on), else the old fields.
        let jobs: [Job]?
        let job: Job?
        let beside: Job?
        let more: [Job]?
    }
    let dir = home.appendingPathComponent("agent")
    let files = ["status.json", "helper.json"].compactMap { (try? Data(contentsOf: dir.appendingPathComponent($0))).flatMap { try? JSONDecoder().decode(File.self, from: $0) } }
    guard let f = files.max(by: { $0.beat < $1.beat }), Int(Date().timeIntervalSince1970) - f.beat < 6 * 60 else { return [] }
    return (f.jobs ?? ([f.job, f.beside].compactMap { $0 } + (f.more ?? []))).map {
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
/// process has ended. False, and this app left as it was, if the copy fails; false too, the new
/// copy in place but this process running on, if the helper that would open it can't start.
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
    // (A launcher signed otherwise no longer matches launchd's launch constraint: the new app,
    // opened, registers the service again before it starts it, BuildService.start.)
    if let a = cdhash(launcherPath(me)), let b = cdhash(launcherPath(tmp)), a != b {
        updateLog("the new app's launcher is signed otherwise (cdhash \(a.prefix(12))… → \(b.prefix(12))…): it registers the build service again when it opens")
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
    // (The old copy stays: the build service's launcher may run from it; the app opened again
    // restarts it from here and deletes the copy once nothing runs from it, removeOldCopies.)
    // Opened again by a helper, the new bundle's own binary (`--reopen`, `reopenHelper`), once this
    // process has ended: it outlives this one, retries until the new app runs, and logs each step.
    let p = Process()
    p.executableURL = Bundle(url: me)?.executableURL ?? me.appendingPathComponent("Contents/MacOS/scenic-status")
    p.arguments = ["--reopen", "\(getpid())", me.path]
    p.standardInput = FileHandle.nullDevice
    p.standardOutput = FileHandle.nullDevice
    p.standardError = FileHandle.nullDevice
    do { try p.run() } catch {
        // (Not quitting then: this process runs on from the copy left aside, the new one in place
        // for the next launch.)
        updateLog("starting the reopen helper: \(error.localizedDescription); this app runs on, the new one opens at its next launch")
        return false
    }
    updateLog("replaced \(me.path) by \(new.path) (version \(bundleVersion(me) ?? "?")); helper pid \(p.processIdentifier) opens it once pid \(getpid()) ends")
    return true
}

/// The update's log (~/Library/Logs/scenic/update.log): each replace and reopen, with the time (the
/// app's own output goes nowhere when LaunchServices opens it). Also NSLog'd.
func updateLog(_ line: String) {
    NSLog("scenic: \(line)")
    let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/scenic")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let f = dir.appendingPathComponent("update.log")
    let fmt = DateFormatter()
    fmt.dateFormat = "yyyy-MM-dd HH:mm:ss.SSS"
    let data = Data("\(fmt.string(from: Date())) [\(getpid())] \(line)\n".utf8)
    // (Kept to its last ~512 KB once it passes 1 MB, cut at a line.)
    if let size = (try? FileManager.default.attributesOfItem(atPath: f.path))?[.size] as? NSNumber, size.intValue > 1 << 20,
       let old = try? Data(contentsOf: f) {
        let tail = old.suffix(512 << 10)
        let kept = tail.firstIndex(of: 10).map { tail[tail.index(after: $0)...] } ?? tail
        try? Data(kept).write(to: f, options: .atomic)
    }
    if let h = FileHandle(forWritingAtPath: f.path) {
        defer { try? h.close() }
        _ = try? h.seekToEnd()
        try? h.write(contentsOf: data)
    } else {
        try? data.write(to: f)
    }
}

/// The seconds waited before each attempt to open the app again. The first lets LaunchServices
/// forget the old process: an `open` sent while it still counts it as running goes to it and fails
/// (procNotFound, -600: 19 Oct on the M1, 0.15 s after the old pid ended; task #427). The rest back
/// off; about two minutes in all.
let reopenDelays: [Double] = [1, 2, 4, 8, 15, 30, 60]

/// The reopen helper's steps, apart from the system so they can be tested (`--test`): waits up to
/// `exitWait` seconds for the old process to end, then for each delay waits, ends a stray copy of
/// the app (`endStray`: what it ended, if any), opens the app unless it already runs, and checks for
/// up to `verifyWait` seconds that it runs. True once it runs; false after the last attempt, or
/// when the old process never ended.
func reopenLoop(oldGone: () -> Bool, endStray: () -> String?, open: () -> (Int32, String), running: () -> Bool,
                sleep: (Double) -> Void, log: (String) -> Void, delays: [Double] = reopenDelays, exitWait: Double = 120,
                verifyWait: Double = 10) -> Bool {
    var waited = 0.0
    while !oldGone() {
        if waited >= exitWait {
            log("the old process still runs after \(Int(exitWait)) s: not opening the new app")
            return false
        }
        sleep(0.2)
        waited += 0.2
    }
    log("the old process ended")
    for (n, d) in delays.enumerated() {
        sleep(d)
        if running() {
            log("attempt \(n + 1): the app runs (opened elsewhere)")
            return true
        }
        if let s = endStray() { log("attempt \(n + 1): \(s)") }
        let (st, out) = open()
        var checked = 0.0
        while checked < verifyWait {
            if running() {
                log("attempt \(n + 1): opened (open exited \(st))")
                return true
            }
            sleep(0.5)
            checked += 0.5
        }
        let said = out.trimmingCharacters(in: .whitespacesAndNewlines)
        log("attempt \(n + 1): not running \(Int(verifyWait)) s after open (exited \(st)\(said.isEmpty ? "" : ": " + said))")
    }
    log("gave up after \(delays.count) attempts: open the app by hand")
    return false
}

/// Whether a process other than this one runs the executable of the app at `app` (its launcher
/// doesn't count).
func appRuns(_ app: String) -> Bool {
    guard let exe = Bundle(path: app)?.executableURL?.path else { return false }
    let want = realPath(exe) ?? exe
    return allPids().contains { $0 != getpid() && executablePath($0).map { realPath($0) ?? $0 } == want }
}

/// Another copy of the app at `app` running as an app (its bundle id, at another path: a build
/// run by hand, as `scenic-status --help` did before it refused unknown flags) ended: `open` only
/// activates a running app of the same bundle id, so the update's app never started (the mini, 22
/// Oct: "not running 10 s after open (exited 0)" seven times). SIGTERM to that exact pid ends it
/// without its quit (which would stop the build service); waited for up to 5 s. What it ended.
func endStrayCopy(_ app: String) -> String? {
    guard let b = Bundle(path: app), let id = b.bundleIdentifier, let exe = b.executableURL?.lastPathComponent,
          let mine = realPath(app) else { return nil }
    var ended: [String] = []
    for pid in allPids() where pid != getpid() {
        guard let p = executablePath(pid).map({ realPath($0) ?? $0 }), p.hasSuffix("/Contents/MacOS/\(exe)") else { continue }
        let bundle = URL(fileURLWithPath: p).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().path
        // (A command-line run, never registered with LaunchServices, doesn't stop `open`: left.)
        guard bundle != mine, Bundle(path: bundle)?.bundleIdentifier == id,
              NSRunningApplication(processIdentifier: pid) != nil, kill(pid, SIGTERM) == 0 else { continue }
        for _ in 0..<25 where kill(pid, 0) == 0 { Thread.sleep(forTimeInterval: 0.2) }
        ended.append("pid \(pid) from \(bundle)")
    }
    return ended.isEmpty ? nil : "ended a stray copy with this app's bundle id (\(ended.joined(separator: ", "))), which open would have activated instead"
}

/// `--reopen <pid> <app>`: the helper an update starts (`replaceSelf`): the app opened again once
/// the old process has ended, retried until it runs (`reopenLoop`), each step in update.log.
/// Exits 0 when it runs.
func reopenHelper(_ pidArg: String, _ app: String) -> Never {
    guard let pid = Int32(pidArg) else {
        updateLog("reopen: bad pid \(pidArg)")
        exit(2)
    }
    let ok = reopenLoop(
        oldGone: { kill(pid, 0) != 0 && errno == ESRCH },
        endStray: { endStrayCopy(app) },
        open:{ run("/usr/bin/open", [app, "--args", "--no-window"]) },
        running: { appRuns(app) },
        sleep: { Thread.sleep(forTimeInterval: $0) },
        log: { updateLog("reopen \(app): \($0)") })
    exit(ok ? 0 : 1)
}

/// `--test <dir>`: the arguments' check (`argsProblem`), `reopenLoop`'s cases against fakes, and
/// `parseJobState` and `spawnBroken` over captured `launchctl print` output (<dir>/launchctl-*.txt:
/// the live job running, its LWCR failure, stopped); prints each and PASS or FAIL, exits 1 on a
/// failure.
func selfTest(_ dir: String) -> Never {
    var failed = 0
    func check(_ name: String, _ ok: Bool) {
        print("\(ok ? "ok" : "FAIL") \(name)")
        if !ok { failed += 1 }
    }
    // The arguments.
    check("no arguments, or LaunchServices' own: the app", argsProblem([]) == nil && argsProblem(["-psn_0_12345"]) == nil)
    check("--help and -h anywhere: the usage", argsProblem(["--help"]) == "" && argsProblem(["-h"]) == "" && argsProblem(["--plan", "--help"]) == "")
    check("an unknown flag refused, never the app", argsProblem(["--hepl"])?.contains("unknown") == true && argsProblem(["plan"])?.contains("unknown") == true)
    check("values counted", argsProblem(["--reopen", "12", "/Applications/Scenic.app"]) == nil && argsProblem(["--reopen", "12"])?.contains("2 values") == true
        && argsProblem(["--show"])?.contains("a value") == true && argsProblem(["--replay", "a.json", "b.json"]) == nil && argsProblem(["--replay"]) != nil
        && argsProblem(["--no-window", "--show", "panel"]) == nil)
    check("one mode at a time, nothing twice", argsProblem(["--print", "--plan"])?.contains("together") == true
        && argsProblem(["--test", "d", "--no-window"])?.contains("together") == true
        && argsProblem(["--show", "menu", "--show", "panel"])?.contains("twice") == true
        && argsProblem(["--menu-proof", "--show", "menu"]) == nil)
    // The hold ask's file, as control.rs's request_hold writes it (its tests read the same files).
    for (held, f) in [(true, "hold-request-pause.json"), (false, "hold-request-resume.json")] {
        let want = (try? Data(contentsOf: URL(fileURLWithPath: dir).appendingPathComponent(f))).flatMap { try? JSONSerialization.jsonObject(with: $0) } as? NSDictionary
        let got = (try? JSONSerialization.jsonObject(with: holdRequestJSON(held, by: "the menu bar on m1", at: 1700000000))) as? NSDictionary
        check("the hold ask's file as scenic writes it (\(f))", want != nil && want == got)
    }
    // The clear ask's file, as room.rs's request_clear writes it (its tests read the same files).
    for (deep, f) in [(false, "clear-request-caches.json"), (true, "clear-request-deep.json")] {
        let want = (try? Data(contentsOf: URL(fileURLWithPath: dir).appendingPathComponent(f))).flatMap { try? JSONSerialization.jsonObject(with: $0) } as? NSDictionary
        let got = (try? JSONSerialization.jsonObject(with: clearRequestJSON(by: "the menu bar on m1", at: 1700000000, deep: deep))) as? NSDictionary
        check("the clear ask's file as scenic writes it (\(f))", want != nil && want == got)
    }
    // The status as the agent serves it (task #846: agent/asks.rs said_status writes the same
    // file): a Mac paused alone with three jobs finishing, printed as said, every job shown.
    let said = (try? Data(contentsOf: URL(fileURLWithPath: dir).appendingPathComponent("status-said.json"))).flatMap { try? JSONDecoder().decode(Reply.self, from: $0) }
    let ownSaid = (try? Data(contentsOf: URL(fileURLWithPath: dir).appendingPathComponent("status-said.json"))).flatMap { d in (try? JSONSerialization.jsonObject(with: d) as? [String: Any])?["status"] }.flatMap { try? JSONSerialization.data(withJSONObject: $0) }.flatMap { try? JSONDecoder().decode(Own.self, from: $0) }
    check("the agent's status reads (status-said.json)", said?.status != nil && ownSaid?.hold != nil)
    check("out of touch: the agent's threshold", outOfTouch(said) == 600 && outOfTouch(nil) == outOfTouchDefault)
    let (kind, line) = classify(said)
    check("the lead's state: its verdict, printed", kind == .paused && line.hasPrefix("Pausing: 3 jobs"))
    let here = state(said, own: ownSaid)
    check("this Mac pausing alone: pausing while its jobs finish, never paused at once", here.0 == .paused && here.1.hasPrefix("Pausing:"))
    let items = holdItems(said, own: ownSaid)
    check("its hold items: the stage, each job finishing, Resume", items.first?.title.hasPrefix("Pausing:") == true && items.filter { $0.title.contains("finishing unit") }.count == 2 && items.last?.held == false)
    let printed = lines(said, line, own: ownSaid).map(\.text)
    check("every job shown, slot by slot", (1...3).allSatisfy { k in printed.contains { $0.hasPrefix("Slot \(k): ") } })
    check("a stuck loop said, each Mac's, by its agent's threshold", printed.contains("m1: loop stuck 6\u{00A0}min, in the pool's round") && printed.contains("m4: loop stuck 10\u{00A0}min") && !printed.contains { $0.hasPrefix("m6: loop stuck") })
    check("a stage's counts as said", printed.contains { $0.contains("roads: 1,234 of 2,000 z6 tiles (12 with nothing to publish)") } && printed.contains { $0.contains("outlines: done") })
    // The same status from an agent before #846 (a rollout: the lead updated last): its verdict,
    // stage and counts worked out from the older fields, never "older app".
    func strip(_ v: Any) -> Any {
        if let d = v as? [String: Any] { return d.filter { !["verdict", "hold", "counts", "count", "finished", "out_of_touch_s", "loop_stuck_s"].contains($0.key) }.mapValues(strip) }
        if let a = v as? [Any] { return a.map(strip) }
        return v
    }
    let old = (try? Data(contentsOf: URL(fileURLWithPath: dir).appendingPathComponent("status-said.json"))).flatMap { try? JSONSerialization.jsonObject(with: $0) }.flatMap { try? JSONSerialization.data(withJSONObject: strip($0)) }.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) }
    let (oldKind, oldLine) = classify(old)
    check("an older lead's state from its older fields", old?.status?.verdict == nil && oldKind == .paused && oldLine.hasPrefix("Paused (from the menu bar on m1)") && outOfTouch(old) == outOfTouchDefault)
    let oldPrinted = lines(old, oldLine).map(\.text)
    check("an older agent's loop never said stuck", !oldPrinted.contains { $0.contains("loop stuck") })
    check("an older lead's counts from its older fields", oldPrinted.contains { $0.contains("roads: 1,234 of 2,000 z6 tiles") } && oldPrinted.contains { $0.contains("outlines: done") } && oldPrinted.contains { $0.contains("unit: 1,234 of 2,000 z6 tiles") })
    // The reopen helper. A fake system: the old process ends after `exitAfter` polls; each open
    // succeeds from attempt `worksFrom` on (earlier ones go to the old process, as procNotFound did,
    // or to a stray copy until it's ended, `stray`); the app runs from the check after a
    // successful open.
    func sim(exitAfter: Int, worksFrom: Int, already: Bool = false, stray: Bool = false) -> (ok: Bool, opens: Int, slept: Double, log: [String]) {
        var polls = 0, opens = 0, runs = already, slept = 0.0, log: [String] = [], strayRuns = stray
        let ok = reopenLoop(
            oldGone: { polls += 1; return polls > exitAfter },
            endStray: { defer { strayRuns = false }; return strayRuns ? "ended a stray copy" : nil },
            open: { opens += 1; if opens >= worksFrom && !strayRuns { runs = true }; return (opens >= worksFrom ? 0 : 1, opens >= worksFrom ? "" : "procNotFound") },
            running: { runs },
            sleep: { slept += $0 },
            log: { log.append($0) },
            delays: [1, 2, 4], exitWait: 5, verifyWait: 2)
        return (ok, opens, slept, log)
    }
    let a = sim(exitAfter: 3, worksFrom: 1)
    check("opens once the old process ends, after the first delay", a.ok && a.opens == 1 && a.slept >= 1.6 && a.log.last?.contains("attempt 1: opened") == true)
    let b = sim(exitAfter: 0, worksFrom: 3)
    check("retries a failed open (procNotFound) and logs it", b.ok && b.opens == 3 && b.log.contains { $0.contains("attempt 1: not running") && $0.contains("procNotFound") })
    let c = sim(exitAfter: 0, worksFrom: 99)
    check("gives up after the last attempt", !c.ok && c.opens == 3 && c.log.last?.contains("gave up after 3") == true)
    let d = sim(exitAfter: 1000, worksFrom: 1)
    check("never opens while the old process runs", !d.ok && d.opens == 0 && d.log.last?.contains("still runs") == true)
    let e = sim(exitAfter: 0, worksFrom: 1, already: true)
    check("doesn't open a second copy when it already runs", e.ok && e.opens == 0)
    check("the real schedule waits before its first attempt and spans about two minutes", reopenDelays.first! >= 1 && reopenDelays.reduce(0, +) >= 100)
    let s = sim(exitAfter: 0, worksFrom: 1, stray: true)
    check("a stray copy (which open would activate) ended before the open", s.ok && s.opens == 1 && s.log.contains { $0.contains("attempt 1: ended a stray copy") })
    check("no stray copy of a missing app", endStrayCopy("/nonexistent/Scenic.app") == nil)
    // Following the app (#799): asked to stop, waited for, never killed while it stops.
    check("a launcher elsewhere: asked to stop; at most once a minute",
          followStep(elsewhere: true, asked: false, ended: false, sinceAsk: 3600) == .stop
          && followStep(elsewhere: true, asked: false, ended: false, sinceAsk: 10) == .wait)
    check("asked and still stopping: waited for (asked again after a minute, never killed)",
          followStep(elsewhere: true, asked: true, ended: false, sinceAsk: 10) == .wait
          && followStep(elsewhere: true, asked: true, ended: false, sinceAsk: 120) == .stop)
    check("asked and ended: started", followStep(elsewhere: true, asked: true, ended: true, sinceAsk: 10) == .start
          && followStep(elsewhere: false, asked: true, ended: true, sinceAsk: 10) == .start)
    check("this app's own launcher: left", followStep(elsewhere: false, asked: false, ended: false, sinceAsk: 3600) == .wait)
    // Stale launchers.
    check("a launcher older than the job's: stale; the job's own and a newer one: not",
          staleLaunchers([(10, 100), (20, 500), (30, 900)], job: (20, 500)) == [10])
    check("this process's start read", processStart(getpid()).map { $0 > 0 } == true)
    // launchd's state.
    func load(_ f: String) -> String { (try? String(contentsOfFile: "\(dir)/launchctl-\(f).txt", encoding: .utf8)) ?? "" }
    let (running, lwcr, stopped) = (load("running"), load("lwcr"), load("stopped"))
    check("the fixtures read", !running.isEmpty && !lwcr.isEmpty && !stopped.isEmpty)
    check("running: its pid, not broken (its coalitions' \"state = active\" ignored)", parseJobState(running) == .running(19704) && spawnBroken(running) == nil)
    check("needs LWCR update: broken, not running", parseJobState(lwcr) == .stopped && spawnBroken(lwcr)?.contains("LWCR") == true)
    let failedOnly = lwcr.replacingOccurrences(of: "needs LWCR update | ", with: "")
    check("spawn failed with exit 78 alone: broken", spawnBroken(failedOnly)?.contains("78") == true)
    check("spawn failed with another exit: not this", spawnBroken(failedOnly.replacingOccurrences(of: "78: EX_CONFIG", with: "1")) == nil)
    check("stopped (the agent quit, exit 76): not broken", parseJobState(stopped) == .stopped && spawnBroken(stopped) == nil)
    check("a nested section's line doesn't count", spawnBroken(running.replacingOccurrences(of: "\t\tname = local.scenic.build", with: "\t\tproperties = needs LWCR update")) == nil)
    print(failed == 0 ? "PASS" : "FAIL (\(failed))")
    exit(failed == 0 ? 0 : 1)
}

/// A process's executable path (proc_pidpath), nil if it can't be read.
func executablePath(_ pid: Int32) -> String? {
    var buf = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
    let n = proc_pidpath(pid, &buf, UInt32(buf.count))
    return n > 0 ? String(cString: buf) : nil
}

/// A path with its symlinks resolved (realpath: /tmp → /private/tmp), nil if it doesn't exist.
func realPath(_ p: String) -> String? {
    guard let r = realpath(p, nil) else { return nil }
    defer { free(r) }
    return String(cString: r)
}

/// `followApp`'s step: a launcher asked to stop waited for until it has ended, then started; one
/// running from elsewhere asked to stop (again after a minute: SIGTERM only, never a kill).
enum Follow: Equatable { case wait, stop, start }
func followStep(elsewhere: Bool, asked: Bool, ended: Bool, sinceAsk: TimeInterval) -> Follow {
    if asked && ended { return .start }
    return elsewhere && sinceAsk > 60 ? .stop : .wait
}

/// Of launchers (pid, start), the stale ones beside the job's own: not it, and started before it.
func staleLaunchers(_ launchers: [(Int32, UInt64)], job: (Int32, UInt64)) -> [Int32] {
    launchers.filter { $0.0 != job.0 && $0.1 < job.1 }.map { $0.0 }
}

/// When a process started (microseconds since the epoch), nil if it can't be read.
func processStart(_ pid: Int32) -> UInt64? {
    var info = proc_bsdinfo()
    let size = Int32(MemoryLayout<proc_bsdinfo>.size)
    guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
    return info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
}

/// Every process's pid.
func allPids() -> [Int32] {
    var pids = [Int32](repeating: 0, count: 8192)
    let n = proc_listallpids(&pids, Int32(pids.count * MemoryLayout<Int32>.size))
    return n > 0 ? pids.prefix(Int(n)).filter { $0 > 0 } : []
}

/// The executables of the processes whose path can be read (this user's).
func runningExecutables() -> [String] { allPids().compactMap { executablePath($0) } }

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
