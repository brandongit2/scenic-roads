// Scenic's menu bar item: the build agent's state at a glance (docs/building.md §2, Status). An icon for
// the state (building, paused, waiting, nothing to do, a problem, the lead out of touch); any
// click on it (left or right) opens its menu: the state in a line, this Mac's downloads
// and build caches, and the controls below. Its timers run in
// the run loop's common modes, so the polls and the icon go on while a menu is
// open. It asks `scenic menu` (crates/agentcore/src/menu.rs, the installed app's), which answers
// from this Mac's disk alone, never the NAS (docs/roles.md §3.4): this Mac's own agent's status,
// and on a member the lead's line as the lead's last beat answer carried it. This Mac's own agent's status, read
// from its file, says what its build caches hold, whether they can be cleared now,
// and the last clears (crates/agentcore/src/agent/room.rs): Clear the build caches asks
// that agent to clear what later jobs copy back cheaply from the NAS, Clear caches deeply that
// and what's costly to make again, at any moment; and this Mac's memory against its floor, as its agent says it (the brake
// first when it's on). (The disk room target and the memory floor are set on the build page.) With
// the pool on (docs/pool.md §11), its agent's view of the pool: who leads (the Mac mini, the fixed
// lead: no item moves it), each Mac and its state.
//
// It's the menu bar item of Scenic.app, the app opened from Launchpad, Spotlight or /Applications
// (and at login), never in the Dock: opening it starts this Mac's build service, quitting it stops that service
// (service.swift). The map server is no part of it: the devices' map is the Mac mini's, a service of
// its own (docs/macs.md, Devices; task #874). Its menu's Open the map on this Mac opens this Mac's
// own, starting a laptop's server on demand (`startMapHere`; docs/roles.md §8.3).
//
//   tools/status/bundle.sh <folder> [sandbox options]   builds Scenic.app (both files, its launcher
//                                                        and its build service's launchd plist)
//   scenic-status --help                    its arguments (`usage` below); an unknown one is refused
//                                           (exit 2), never the app started
// SCENIC_STATUS_COMMAND overrides the program asked (`<home>/app/current/scenic`; else Info.plist's
// ScenicStatusCommand), run with `menu`,
// SCENIC_HOME the app folder (else Info.plist's ScenicHome).

import AppKit
import SystemConfiguration
import ServiceManagement

let home = (ProcessInfo.processInfo.environment["SCENIC_HOME"] ?? info["ScenicHome"] as? String).map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/scenic")
let statusCommand = ProcessInfo.processInfo.environment["SCENIC_STATUS_COMMAND"] ?? info["ScenicStatusCommand"] as? String ?? home.appendingPathComponent("app/current/scenic").path

/// The status now, as `scenic menu` prints it (nil when it can't be run, fails or says nothing
/// readable within `timeout` seconds: a disk or a process start held up). Never on the main thread.
func fetchReply(timeout: TimeInterval = 10) -> Reply? {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: statusCommand)
    p.arguments = ["menu", "--home", home.appendingPathComponent("agent").path]
    let out = Pipe()
    p.standardOutput = out
    p.standardError = FileHandle.nullDevice
    do { try p.run() } catch { return nil }
    // (Ended at the timeout, and killed 2 s later if it's still there: one poll at a time waits
    // on it, so a hung read never piles them up, and never holds the polls for long.)
    let pid = p.processIdentifier
    let killer = DispatchWorkItem {
        guard p.isRunning else { return }
        p.terminate()
        DispatchQueue.global().asyncAfter(deadline: .now() + 2) { if p.isRunning { kill(pid, SIGKILL) } }
    }
    DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: killer)
    let data = out.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    killer.cancel()
    guard p.terminationStatus == 0 else { return nil }
    return try? JSONDecoder().decode(Reply.self, from: data)
}
/// Without a heartbeat for this long, a Mac counts as out of touch (asleep, off, away, or its
/// agent stuck): the agent's own threshold, served in its status (`out_of_touch_s`, task #846:
/// crates/agentcore/src/pool/beat.rs OUT_OF_TOUCH_S), this from an agent before it.
let outOfTouchDefault = 600

/// The out-of-touch threshold of an answer's status.
func outOfTouch(_ r: Reply?) -> Int { r?.status?.out_of_touch_s ?? outOfTouchDefault }

// What `scenic menu` answers (crates/agentcore/src/menu.rs): this Mac's agent's status
// (crates/agentcore/src/agent/mod.rs Status), and on a member its lead's line.
struct Reply: Decodable {
    let status: Status?
    let now: Int
    let log: String?
    /// This Mac paused alone (crates/jobrt/src/control.rs Hold, task #224), and its ask while
    /// the agent hasn't taken it up (servers from #224 on).
    let held_here: HeldInfo?
    let hold_asked: Bool?
    /// A member's: the lead's line as the lead's last beat answer carried it (docs/roles.md §3.4;
    /// none on the lead, and from a `scenic menu` before it).
    let lead: LeadView?
}

/// The lead's line as this member last heard it (crates/agentcore/src/coord/beats.rs LeadView):
/// when the lead last answered its beat (0: not yet), and its line (none from an older lead).
struct LeadView: Decodable {
    let at: Int
    let lead: LeadLine?
}

/// The build in a line, from the lead (crates/agentcore/src/coord/held.rs LeadLine): its Mac, its
/// app, the build's state as it says it, the build's pause.
struct LeadLine: Decodable {
    let host: String
    let app: String
    let said: Verdict
    let pause: PauseInfo?
    /// What needs a look, as the lead's status flags it (crates/agentcore/src/agent/flags.rs).
    let flags: [Flag]?
}

/// A flag of the lead's status (crates/agentcore/src/agent/flags.rs Flag): "idle", "stuck", "slow", "stale" or "repeat", the
/// Mac it's about, what it says.
struct Flag: Decodable {
    let kind: String
    let mac: String
    let said: String
}

/// The build's flags: the lead's word on a member, else (the lead) this Mac's status's.
func buildFlags(_ r: Reply?) -> [Flag] {
    if let l = r?.lead?.lead { return l.flags ?? [] }
    return r?.status?.flags ?? []
}

/// The build's pause: the lead's word on a member, else (the lead, or no line heard) this Mac's
/// status's.
func buildPause(_ r: Reply?) -> PauseInfo? {
    if let l = r?.lead?.lead { return l.pause }
    return r?.status?.pause
}

/// One Mac paused alone (crates/jobrt/src/control.rs Hold): who asked, since when.
struct HeldInfo: Decodable {
    let by: String
    let at: Int
}

struct Status: Decodable {
    let host: String
    let app: String?
    let beat: Int
    let conditions: Conditions
    /// Its running jobs in slot order (agents from 2026-10-18 on); an older agent's, the old
    /// fields below (its first slot's, its second's beside it, the rest), which a newer one writes
    /// only while a Mac of the pool runs an older app. Read through `allJobs`.
    let jobs: [Job]?
    let job: Job?
    let beside: Job?
    let more: [Job]?
    let waiting: [Waiting]
    let recent: [Done]
    let built: [String: Built]?
    /// Publishing as planned (crates/pipeline/src/agent/build.rs PublishLine): its regions done but
    /// not yet recorded built, each with why.
    let publishing: PublishingInfo?
    /// The build to the end (agents from 2026-10-03 on).
    let checklist: [StageLine]?
    /// Other Macs helping, building tiles (agents from 2026-10-04 on).
    let helpers: [Helper]?
    /// The build's pause, while it's paused (agents from 2026-10-05 on).
    let pause: PauseInfo?
    /// The regions (their names), and when the build will be done and the map next updated (the
    /// forecast: crates/agentcore/src/agent/forecast.rs; agents from 2026-10-05 on).
    let regions: [Recipe]?
    let forecast: Forecast?
    /// What needs a look (the lead's: crates/agentcore/src/agent/flags.rs).
    let flags: [Flag]?
    /// The owner's asks this Mac passed on that the lead refused, not yet seen on the build page
    /// (crates/agentcore/src/agent/answered.rs; agents from task #922 on).
    let refused: [RefusedAsk]?

    /// Its running jobs, every slot's in order (`jobs`, else the old fields).
    var allJobs: [Job] { jobs ?? ([job, beside].compactMap { $0 } + (more ?? [])) }
    /// The gate's units (docs/inputs.md §4.7; agents from #134 on): a held one badges the icon.
    let inputs: [InputUnit]?
    /// Its Mac's own pause's stage, its state in a word and a line, and the out-of-touch threshold,
    /// as its agent says them (task #846: crates/agentcore/src/agent/mod.rs Status::say); the menu
    /// prints them, working none of them out.
    let hold: HoldStage?
    let verdict: Verdict?
    /// The lead's: the build in a line, a member building counting as building (Status::build_said,
    /// the line its members are told); none from an older agent.
    let build: Verdict?
    let out_of_touch_s: Int?
    /// This Mac paused alone (task #224): what an agent before #846 says of its hold.
    let held: HeldInfo?
    /// Its loop's round's age, the threshold past which it's stuck, the phase it's in: `loopStuck`.
    let loop_age_s: Int?
    let loop_stuck_s: Int?
    let loop_phase: String?

    /// Its verdict; an agent's before #846 (a rollout: the pool's Macs update one by one), worked
    /// out from its older fields as crates/agentcore/src/agent/mod.rs Status::verdict_of does.
    var said: Verdict {
        if let v = verdict { return v }
        let jobs = allJobs
        let n = { (k: Int) in "\(k) job\(k == 1 ? "" : "s")" }
        if let h = held { return Verdict(state: jobs.contains { $0.paused == nil } ? "pausing" : "paused", said: "Paused (from \(h.by)); the other Macs go on") }
        if jobs.contains(where: { $0.paused == nil && $0.pausing == nil }) { return Verdict(state: "building", said: jobs.count == 1 ? "Building: \(jobs[0].what)" : "Building: \(n(jobs.count))") }
        let stopping = jobs.filter { $0.paused == nil && $0.pausing != nil }.count
        if stopping > 0 { return Verdict(state: "stopping", said: "Pausing: \(n(stopping)) finishing what \(stopping == 1 ? "it's" : "they're") on") }
        if let p = jobs.first(where: { $0.paused != nil })?.paused { return Verdict(state: "frozen", said: "Paused: \(p)") }
        if let p = pause { return Verdict(state: "paused", said: "Paused with the build (from \(p.by))") }
        if let w = waiting.first(where: { $0.why.contains("failed") }) { return Verdict(state: "failed", said: "Failed: \(w.what)") }
        if let d = recent.first, !d.ok { return Verdict(state: "failed", said: "Failed: \(d.what)") }
        if let w = waiting.first { return Verdict(state: "waiting", said: "Waiting: \(w.what)") }
        return Verdict(state: "idle", said: "Nothing to build")
    }
}

/// An ask the lead refused (crates/agentcore/src/agent/answered.rs Answered): what was asked, when,
/// and the lead's reason.
struct RefusedAsk: Decodable {
    let what: String
    let at: Int
    let refused: String?
}

/// A Mac's own pause's stage (crates/jobrt/src/control.rs HoldView): "asked", "pausing" (its
/// jobs finishing, by id), "paused", "resuming"; whether it goes to paused; who asked, since when;
/// the stage in words.
struct HoldStage: Decodable {
    let stage: String
    let pause: Bool
    let by: String?
    let since: Int?
    let finishing: [String]?
    let said: String
}

/// A Mac's state as its agent says it (crates/agentcore/src/agent/mod.rs Verdict): a word and a line.
struct Verdict: Decodable {
    let state: String
    let said: String
}

/// A gate unit (crates/inputs/src/inputs/view.rs InputView).
struct InputUnit: Decodable {
    let unit: String
    /// "ok", "checking" or "held" (held while it's checked again too: `checking`).
    let state: String
    let checking: Bool?
    let held: [String]?
    let findings: [InputFinding]?
}

/// A finding holding a gate unit's change (crates/inputs/src/inputs/mod.rs Finding).
struct InputFinding: Decodable {
    let id: String
    let level: String
    let message: String
}

/// The units the gate holds now.
func heldInputs(_ r: Reply?) -> [InputUnit] {
    (r?.status?.inputs ?? []).filter { $0.state == "held" }
}

/// A held unit in a line: "regions: 1 warning held, 1 error (wales.toml)".
func inputLine(_ v: InputUnit) -> String {
    let fs = v.findings ?? []
    let w = fs.filter { $0.level == "warning" }.count, e = fs.count - fs.filter { $0.level == "warning" }.count
    var parts: [String] = []
    if w > 0 { parts.append("\(w) warning\(w == 1 ? "" : "s") held") }
    if e > 0 { parts.append("\(e) error\(e == 1 ? "" : "s")") }
    return "\(v.unit): \(parts.isEmpty ? "held" : parts.joined(separator: ", "))\((v.held ?? []).isEmpty ? "" : " (\((v.held ?? []).joined(separator: ", ")))")\(v.checking == true ? ", checking again" : "")"
}

/// The icon: the build's state's symbol, with a small warning triangle while an input is held
/// (the build goes on with the last good version meanwhile).
func iconImage(_ kind: Kind, held: Bool, line: String) -> NSImage? {
    guard let base = NSImage(systemSymbolName: kind.symbol, accessibilityDescription: line) else { return nil }
    guard held, let badge = NSImage(systemSymbolName: "exclamationmark.triangle.fill", accessibilityDescription: "an input held") else {
        base.isTemplate = true
        return base
    }
    let size = NSSize(width: 18, height: 16)
    let img = NSImage(size: size, flipped: false) { r in
        base.draw(in: NSRect(x: 0, y: 2, width: 14, height: 14))
        // (Cleared under the badge so it reads over the symbol.)
        NSGraphicsContext.current?.compositingOperation = .clear
        NSBezierPath(ovalIn: NSRect(x: 8, y: -1, width: 11, height: 10)).fill()
        NSGraphicsContext.current?.compositingOperation = .sourceOver
        badge.draw(in: NSRect(x: 9, y: 0, width: 9, height: 8))
        return true
    }
    img.isTemplate = true
    img.accessibilityDescription = line + " (an input held)"
    return img
}

/// A --replay file's this Mac's agent's own status and whether an ask to clear its caches waits,
/// beside its answer.
struct Replay: Decodable {
    let own: Own?
    let clear_asked: Bool?
}

/// This Mac's own agent's status, as it writes it in its folder (the lead's `status.json`, a
/// helper's `helper.json`), for its build caches.
struct Own: Decodable {
    let host: String
    /// The app its agent runs (the status's `app`).
    let app: String?
    let beat: Int
    let caches: Caches?
    /// This Mac in the pool, while it's on (crates/agentcore/src/agent/mod.rs PoolView).
    let pool: OwnPool?
    /// This Mac's own pause's stage and the out-of-touch threshold (task #846).
    let hold: HoldStage?
    let out_of_touch_s: Int?
    /// This Mac's memory against its floor, as its agent says it (task #932).
    let memory: OwnMemory?

    init(host: String, app: String? = nil, beat: Int, caches: Caches?, pool: OwnPool?, hold: HoldStage?, out_of_touch_s: Int?, memory: OwnMemory? = nil) {
        (self.host, self.app, self.beat, self.caches, self.pool, self.hold, self.out_of_touch_s, self.memory) = (host, app, beat, caches, pool, hold, out_of_touch_s, memory)
    }

    private enum CodingKeys: String, CodingKey { case host, app, beat, caches, pool, hold, out_of_touch_s, memory }

    /// The pool part read on its own: one this app can't read (an agent of another app's, a
    /// rollout) leaves it out, not the whole status.
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        host = try c.decode(String.self, forKey: .host)
        app = try c.decodeIfPresent(String.self, forKey: .app)
        beat = try c.decode(Int.self, forKey: .beat)
        caches = try c.decodeIfPresent(Caches.self, forKey: .caches)
        pool = try? c.decodeIfPresent(OwnPool.self, forKey: .pool)
        hold = try c.decodeIfPresent(HoldStage.self, forKey: .hold)
        out_of_touch_s = try c.decodeIfPresent(Int.self, forKey: .out_of_touch_s)
        memory = try? c.decodeIfPresent(OwnMemory.self, forKey: .memory)
    }
}

/// This Mac's memory against its floor (crates/agentcore/src/agent/memguard.rs View, task #932): its
/// line as the agent words it (available, what compressing frees among it, the floor, what's
/// reserved, why no job starts: the brake, the floor, a give-back's cool-down),
/// the menu printing it as said; and the memory brake, when it's on.
struct OwnMemory: Decodable {
    let said: String
    let brake: String?
}

/// This Mac in the pool: its member, and the pool as the controls show it (docs/pool.md §10, §11;
/// crates/agentcore/src/agent/lead.rs View; agents from 2026-10-08 on).
struct OwnPool: Decodable {
    let member: String
    let lead: PoolView?
}

struct PoolView: Decodable {
    let at: Int
    let lead: LeadOf?
    let leading: Bool
    let members: [PoolMember]
    let no_lead: String?
    /// No lead for an hour or more (or a lead its take-up refused by the journal GC's marker, #651):
    /// since when, why, and what to do (agents from #195 on).
    let alert: String?
    /// Who can't reach whom, each with why (agents from #198 on: docs/pool.md §6.8).
    let unreached: [String]?
    let change: LeadChange?
}

/// The declared lead: `since`, when it took the records up, said by the lead's own agent alone.
struct LeadOf: Decodable {
    let member: String
    let host: String
    let app: String
    let since: Int?
}

struct PoolMember: Decodable {
    let member: String
    let host: String
    let app: String
    let beat: Int
    let me: Bool?
    let leads: Bool?
    let state: String
    let out_of_touch: Bool?
    let away: Bool?
}

struct LeadChange: Decodable {
    let at: Int
    let said: String
}

/// The pool's lines for the menu: who leads, each member and its state, the last change of lead.
func poolLines(_ v: PoolView, now: Int) -> [Line] {
    var out = [Line(text: "", style: .separator)]
    if let why = v.no_lead {
        out.append(Line(text: "The pool · no lead: \(why)", style: .header))
        if let a = v.alert { out.append(Line(text: a, style: .plain)) }
    } else if let l = v.lead {
        let since = l.since.map { " since \(clock($0))" } ?? ""
        out.append(Line(text: "The pool · \(l.host) leads\(v.leading ? " (this Mac)" : "")\(since)", style: .header))
        // (A lead its take-up refused by the journal GC's marker, #651: the alert, what to do.)
        if let a = v.alert { out.append(Line(text: a, style: .plain)) }
    }
    for m in v.members {
        let who = m.me == true ? "\(m.host) (this Mac)" : m.host
        let lead = m.leads == true ? " · leads" : ""
        let heard = m.out_of_touch == true && m.beat > 0 ? ", last heard \(duration(now - m.beat)) ago" : ""
        out.append(Line(text: "\(who)\(lead): \(m.state)\(heard)", style: .small))
    }
    for u in v.unreached ?? [] {
        out.append(Line(text: u, style: .plain))
    }
    if !(v.unreached ?? []).isEmpty {
        out.append(Line(text: "On the Mac named: check it is awake, on the network and its agent running; then allow Scenic in System Settings › Network › Firewall (Options…), or turn the firewall off", style: .small))
    }
    if let c = v.change, now - c.at < 24 * 3600 {
        out.append(Line(text: "\(c.said) (\(clock(c.at)))", style: .small))
    }
    return out
}

/// This Mac's build caches (crates/agentcore/src/agent/room.rs Caches): what a clear and a deep
/// clear would free (bytes, and cache by cache with how each comes back), why they can't be cleared
/// now (none: they can), and the last clear of each level.
struct Caches: Decodable {
    let clearable: Int?
    let clearable_deep: Int?
    let each: [Gone]?
    let why_not: String?
    let cleared: Freed?
    let cleared_deep: Freed?

    /// What a clear of that level would free.
    func clearable(deep: Bool) -> Int? { deep ? clearable_deep : clearable }
}

/// A cache a clear would empty (room.rs Gone): its name in words, its bytes, how it comes back,
/// and whether only a deep clear empties it.
struct Gone: Decodable {
    let what: String
    let bytes: Int
    let back: String
    let deep: Bool?
}

/// What a clear did (room.rs Freed): when, the bytes freed by cache, and for a clear asked for when
/// it couldn't be, why not.
struct Freed: Decodable {
    let at: Int
    let freed: [String: Int]?
    let why_not: String?

    var bytes: Int { (freed ?? [:]).values.reduce(0, +) }
}

struct Recipe: Decodable {
    let id: String
    let name: String
}

/// The forecast's part the menu shows: when it'll all be done (and the range), and when each region
/// is all on the map as it's built now (its `map_at`: its last z6 tile, its map tiles drawn, a catalogue
/// within the minute).
struct Forecast: Decodable {
    let done_at: Int?
    let range: [Int]?
    let regions: [RegionFc]?
    /// Why there's no finish to forecast (nothing left; a new pass first; the units waiting).
    let why: String?

    /// The region (or regions, at the same time) next all on the map, and when.
    var nextOnMap: (at: Int, ids: [String])? {
        let timed = (regions ?? []).compactMap { r in r.map_at.map { (r.id, $0) } }
        guard let at = timed.map({ $0.1 }).min() else { return nil }
        return (at, timed.filter { $0.1 == at }.map { $0.0 })
    }

    func mapAt(_ id: String) -> Int? { regions?.first { $0.id == id }?.map_at }
}

/// A region's forecast: when all of it is on the map as it's built now.
struct RegionFc: Decodable {
    let id: String
    let map_at: Int?
}

/// The build's pause (crates/jobrt/src/control.rs Pause): at the next safe point ("drain") or
/// at once ("freeze"), who asked, since when.
struct PauseInfo: Decodable {
    let mode: String
    let by: String
    let at: Int
}

/// A member, as the lead's agent last heard its status (the lead's status's `helpers`).
struct Helper: Decodable {
    let host: String
    let beat: Int
    /// Its running jobs (a newer agent's), else its first slot's (`job`).
    let jobs: [Job]?
    let job: Job?
    /// Its running jobs, every slot's (an older agent's: its first).
    var allJobs: [Job] { jobs ?? (job.map { [$0] } ?? []) }
    /// Its loop's round's age, the threshold past which it's stuck, the phase it's in: `loopStuck`.
    let loop_age_s: Int?
    let loop_stuck_s: Int?
    let loop_phase: String?
}

/// A Mac's loop stuck (task #899): its round older than its agent's own watchdog threshold
/// (`loop_stuck_s`, crates/agentcore/src/agent/beat.rs LOOP_STUCK), with the phase it's in; nil
/// while it isn't, and from an agent that doesn't say both (before #871 and #899). Its status still
/// beats on a timer of its own (crates/agentcore/src/agent/beat.rs), so it isn't out of touch. The
/// age as of `now`: as the beat (at `beat`) said it, plus the time since.
func loopStuck(age: Int?, limit: Int?, phase: String?, beat: Int, now: Int) -> String? {
    guard let said = age, let limit = limit, limit > 0 else { return nil }
    let age = said + max(0, now - beat)
    guard age > limit else { return nil }
    return "loop stuck \(duration(age))\(phase.map { ", in \($0)" } ?? "")"
}

/// A stage of the build to the end (docs/pipeline.md §4; agents before it: a group of steps): its
/// targets done of total (total unknown until an earlier step makes it) in its noun, or
/// how many are left; its steps' lines (`items`, agents from 2026-10-17 on); its jobs left by name
/// (`next`, the first the one under way), and why it waits or which Mac is on it (`note`).
struct StageLine: Decodable {
    let what: String
    let steps: [String]
    let done: Int?
    let total: Int?
    let left: Int?
    let items: [Item]?
    let next: [String]?
    let note: String?
    /// Which of its jobs a helper may do: "all", or the parts by name.
    let shared: String?

    /// Its counts in words and whether it's done, as the agent says them (build.rs Line::said, task
    /// #846: the build page and `scenic status` say the same).
    let counts: [String]?
    let finished: Bool?
    /// (An agent's before #846, a rollout's: worked out from the older fields as build.rs Line
    /// `finished` and `counts` do.)
    var isDone: Bool { finished ?? (left == 0 || (left == nil && total.map { (done ?? 0) >= $0 } == true)) }
    var said: [String] {
        if let c = counts { return c }
        var out: [String] = []
        var nouns: [String] = []
        let items = self.items ?? []
        for i in items where !nouns.contains(i.noun) {
            nouns.append(i.noun)
            let of = items.filter { $0.noun == i.noun }
            if of.contains(where: { $0.total != nil }) {
                if of.allSatisfy({ $0.total != nil }) {
                    let t = of.reduce(0) { $0 + ($1.total ?? 0) }
                    if t > 0 || out.isEmpty { out.append("\(commas(of.reduce(0) { $0 + ($1.done ?? 0) })) of \(commas(t)) \(i.noun)s") }
                } else { out.append("\(i.noun)s to come") }
            } else if of.allSatisfy({ $0.left != nil }) {
                let l = of.reduce(0) { $0 + ($1.left ?? 0) }
                if l > 0 { out.append("\(commas(l)) \(i.noun)\(l == 1 ? "" : "s") left") }
            } else { out.append("\(i.noun)s to come") }
        }
        return out.isEmpty ? [isDone ? "done" : "to come"] : out
    }
}

/// A step's line of a stage (docs/pipeline.md §4): its targets done of total, or left.
struct Item: Decodable {
    /// Its step's id, the name it's shown by (task #659), and the noun its targets are counted in.
    let step: String
    let noun: String
    let done: Int?
    let total: Int?
    let left: Int?
    /// Its count in words and whether it's done, as the agent says them (build.rs Item, task #846).
    let count: String?
    let finished: Bool?
    /// (An agent's before #846, a rollout's: worked out as build.rs Item::count does.)
    var isDone: Bool { finished ?? (left == 0) }
    var said: String {
        if let c = count { return c }
        if let t = total, t > 0 { return "\(commas(done ?? 0)) of \(commas(t)) \(noun)s" }
        if let l = left { return l == 0 ? "done" : "\(commas(l)) \(noun)\(l == 1 ? "" : "s") left" }
        return "to come"
    }
}

struct JobProgress: Decodable {
    let done: Double
    let total: Double
    /// The noun its count is in, plural ("z6 tiles"): sent as "unit", read as "nouns" too (task
    /// #858: the agent sends "nouns" once no menu bar reading only "unit" can run).
    let nouns: String
    let eta_s: Int?
    /// When it last moved on (seconds since the epoch; absent from older heartbeats).
    let moved_at: Int?

    enum CodingKeys: String, CodingKey { case done, total, nouns, unit, eta_s, moved_at }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        done = try c.decode(Double.self, forKey: .done)
        total = try c.decode(Double.self, forKey: .total)
        nouns = try c.decodeIfPresent(String.self, forKey: .nouns) ?? c.decode(String.self, forKey: .unit)
        eta_s = try c.decodeIfPresent(Int.self, forKey: .eta_s)
        moved_at = try c.decodeIfPresent(Int.self, forKey: .moved_at)
    }
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
    /// Why it's stopping at its next safe point (the build pausing), while it is.
    let pausing: String?
    /// What it waits on, while it waits on a worker holding its unit's tail (its room free beside it).
    let waiting: String?
    let tail: String?
    let progress: JobProgress?
    /// Its parts, in order (a job of more than one says them), and the one it's on.
    let parts: [String]?
    let part: Int?
    /// Its slot (0: the first; an older agent's: none).
    let slot: Int?
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
}

struct PublishingInfo: Decodable {
    let regions: [PublishRegion]?
}

struct PublishRegion: Decodable {
    let id: String
    let kinds: [String]
}

struct Built: Decodable {
    let built: Int
    let total: Int
}

enum Kind {
    case unknown, outOfTouch, building, paused, pausedHere, waiting, idle, problem

    var symbol: String {
        switch self {
        case .unknown: return "questionmark.circle"
        case .outOfTouch: return "moon.zzz"
        case .building: return "gearshape.fill"
        case .paused: return "pause.circle"
        // (This Mac paused alone, `scenic pause --here`: filled, the whole build's outlined.)
        case .pausedHere: return "pause.circle.fill"
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

/// A time to come: "16:40" today, "Tue 07:50" within the week, else "8 Oct 07:50".
func soon(_ t: Int) -> String {
    let d = Date(timeIntervalSince1970: TimeInterval(t))
    let f = DateFormatter()
    f.dateFormat = Calendar.current.isDateInToday(d) ? "HH:mm" : abs(d.timeIntervalSinceNow) < 6 * 86400 ? "EEE HH:mm" : "d MMM HH:mm"
    return f.string(from: d)
}

/// Home folders as "~" (the lead's paths, as its log shows them).
func tildes(_ s: String) -> String {
    s.replacingOccurrences(of: "/Users/[^/ ]+/", with: "~/", options: .regularExpression)
}

func clip(_ s: String, _ n: Int = 80) -> String {
    s.count <= n ? s : String(s.prefix(n - 1)) + "…"
}

/// `s` with its first letter lower case, for a phrase inside a sentence-case menu item ("Cancel
/// building unit 6/1/1"); a first word with another capital (a name: "OSM pass", "OpenStreetMap
/// pass") kept.
func lowerFirst(_ s: String) -> String {
    guard let f = s.first, f.isUppercase, !s.dropFirst().prefix(while: { !$0.isWhitespace }).contains(where: \.isUppercase) else { return s }
    return f.lowercased() + s.dropFirst()
}

/// The helpers heard from lately (one not heard from for `outOfTouch` is left out).
func freshHelpers(_ r: Reply) -> [Helper] {
    (r.status?.helpers ?? []).filter { r.now - $0.beat <= outOfTouch(r) }
}

/// The icon's kind for a state as an agent words it (task #846).
func kindOf(_ state: String) -> Kind {
    switch state {
    case "building": return .building
    case "asked", "pausing", "paused", "stopping", "frozen", "resuming": return .paused
    case "failed": return .problem
    case "waiting": return .waiting
    default: return .idle
    }
}

/// The state, with a line saying it: this Mac's own while it builds; on a member, else, the build
/// as its lead last said it (docs/roles.md §3.4); on the lead, the build as it says it
/// (`Status.build`: its members' lines say the same), else (an older agent) its own word.
func classify(_ r: Reply?) -> (Kind, String) {
    guard let r = r else { return (.unknown, "No status: this Mac's `scenic menu` didn't answer") }
    guard let s = r.status else { return (.unknown, "No status from this Mac's agent (is it running?)") }
    if r.now - s.beat > outOfTouch(r) { return (.outOfTouch, "This Mac's agent out of touch since \(clock(s.beat))") }
    let v = s.said
    let kind = kindOf(v.state)
    if let lv = r.lead {
        if kind == .building { return (kind, clip(v.said)) }
        guard lv.at > 0 else { return (.unknown, "No word from the lead yet") }
        if r.now - lv.at > outOfTouch(r) { return (.outOfTouch, clip("Lead\(lv.lead.map { " (\($0.host))" } ?? "") out of touch since \(clock(lv.at))")) }
        guard let l = lv.lead else { return (kind, clip(v.said)) }
        return (kindOf(l.said.state), clip(l.said.said))
    }
    let b = s.build ?? v
    return (kindOf(b.state), clip(b.said))
}

/// The state with a line saying it, this Mac's agent's view of the pool considered: no lead for an
/// hour or more (its `alert`, fresh) is a problem, whatever the last status says, and the line says
/// why and what to do (docs/pool.md §10).
func state(_ r: Reply?, own: Own?) -> (Kind, String) {
    let now = r?.now ?? Int(Date().timeIntervalSince1970)
    // (This Mac paused alone, task #224: what this Mac does, whatever the pool's state, #628; its
    // stage as its agent says it, #846: asked, pausing while its jobs finish, paused, resuming.)
    if let o = own, now - o.beat <= (o.out_of_touch_s ?? outOfTouchDefault), let h = o.hold {
        return (h.stage == "paused" ? .pausedHere : .paused, clip(h.said))
    }
    // (An agent's before #846, a rollout's: its hold alone.)
    if own?.hold == nil, r?.held_here != nil { return (.pausedHere, "This Mac is paused; the other Macs go on") }
    if let o = own, now - o.beat <= (o.out_of_touch_s ?? outOfTouchDefault), let a = o.pool?.lead?.alert { return (.problem, a) }
    return classify(r)
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
/// "12,345", whatever the Mac's locale: a count as the agent says it (build.rs grouped), for an
/// older agent's counts worked out here.
func commas(_ v: Int) -> String {
    let s = String(v)
    return s.enumerated().map { i, c in i > 0 && (s.count - i) % 3 == 0 ? ",\(c)" : "\(c)" }.joined()
}

func grouped(_ v: Double) -> String {
    let f = NumberFormatter()
    f.numberStyle = .decimal
    f.maximumFractionDigits = 0
    return f.string(from: NSNumber(value: v)) ?? "\(Int(v))"
}

/// A job's progress in words: "40% · 2.4 of 6 z6 tiles · about 12 min left" (the item under way
/// counted by how much of it is done; one item alone, its share), and when it hasn't moved on for a
/// quarter of an hour, since when.
func progressText(_ p: JobProgress, paused: Bool, now: Int) -> String {
    let frac = p.total > 0 ? min(1, p.done / p.total) : 0
    var t = "\(Int((frac * 100).rounded(.down)))%"
    if p.total == 1 {
        t += " · \(p.nouns)"
    } else {
        // (Down, not to the nearest: 2.96 of 6 is 2.9, not 3.)
        let whole = p.done.rounded(.down) == p.done || p.total > 100
        let done = whole ? grouped(p.done.rounded(.down)) : String(format: "%.1f", (p.done * 10).rounded(.down) / 10)
        t += " · \(done) of \(grouped(p.total)) \(p.nouns)"
    }
    if let e = p.eta_s, !paused { t += " · about \(duration(e)) left" }
    if let m = p.moved_at, !paused, now - m >= 15 * 60 { t += " · no further for \(duration(now - m))" }
    return t
}

/// The hold ask's file as `scenic pause --here` / `scenic resume --here` write it
/// (crates/jobrt/src/control.rs request_hold, HoldRequest: no `member` for this Mac). Pinned on
/// both sides by tools/status/test/hold-request-*.json (selfTest, control.rs's tests).
func holdRequestJSON(_ held: Bool, by: String, at: Int) -> Data {
    let h: Any = held ? ["by": by, "at": at] as [String: Any] : NSNull()
    return (try? JSONSerialization.data(withJSONObject: ["held": h, "at": at], options: [.sortedKeys])) ?? Data()
}

/// The menu's items for this Mac paused alone (`scenic pause --here`, task #224): the ask while the
/// agent hasn't taken it up, then its stage as the agent says it (task #846: pausing with each job
/// finishing, paused since when, resuming) and Resume this Mac, else Pause this Mac. `held`: what
/// pressing it asks (pause: true, resume: false), nil for a line only.
func holdItems(_ r: Reply?, own: Own?) -> [(title: String, held: Bool?, tip: String)] {
    var out: [(title: String, held: Bool?, tip: String)] = []
    let h = own?.hold
    if r?.hold_asked == true && h?.stage != "asked" { out.append(("Asked… (this Mac's agent takes it up within seconds)", nil, "")) }
    if let h = h {
        out.append((clip(h.said, 90) + (h.stage == "paused" ? " since \(clock(h.since ?? 0))" : ""), nil, ""))
        for j in h.finishing ?? [] { out.append(("    finishing \(j)", nil, "")) }
    } else if let held = r?.held_here {
        out.append(("Paused since \(clock(held.at)) (from \(clip(held.by, 40)))", nil, ""))
    }
    if h?.pause ?? (r?.held_here != nil) {
        out.append(("Resume this Mac", false, "This Mac takes work again, as `scenic resume --here` does"))
    } else {
        out.append(("Pause this Mac", true, "Only this Mac: its jobs stop at their next safe point and are given back, it takes no new work (the lead goes on granting the others' work); the other Macs go on building"))
    }
    return out
}

/// The menu's lines for an answer, under the state's line (and this Mac's caches, from its agent's
/// own status).
func lines(_ r: Reply?, _ line: String, own: Own? = nil) -> [Line] {
    var out = [Line(text: line, style: .title)]
    guard let r = r, let s = r.status else { return out }
    // When it'll be done and the map next gets new data (as the build page and the map say it).
    if let f = s.forecast, r.now - s.beat <= outOfTouch(r) {
        var t = f.done_at.map { "Done ≈ \(soon($0))" } ?? "No finish to forecast: \(f.why ?? "unknown")"
        if let rg = f.range, rg.count == 2, f.done_at != nil { t += " (\(soon(rg[0]))–\(soon(rg[1])))" }
        out.append(Line(text: t, style: .small))
        if let next = f.nextOnMap {
            let names = Dictionary((s.regions ?? []).map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
            let which = next.ids.prefix(3).map { names[$0] ?? $0 }.joined(separator: ", ") + (next.ids.count > 3 ? " and \(next.ids.count - 3) more" : "")
            out.append(Line(text: "Next on the map ≈ \(soon(next.at)): \(which)", style: .small))
        }
    }
    // (A member's: the build as its lead last said it, and when.)
    if let lv = r.lead, let l = lv.lead {
        out.append(Line(text: "The lead (\(l.host)): \(l.said.said) · heard from \(duration(r.now - lv.at)) ago", style: .small))
    }
    if let p = buildPause(r) {
        let how = p.mode == "freeze" ? "every Mac's job frozen where it was" : "every Mac's job stops at its next safe point"
        out.append(Line(text: "From \(p.by), \(clock(p.at)): \(how); nothing new starts until it's resumed", style: .small))
    }
    // What needs a look, as the lead flags it (the same lines as `scenic status` and the build page).
    for f in buildFlags(r) {
        out.append(Line(text: "\(f.kind.capitalized): \(f.said)", style: .small))
    }
    // This Mac's asks the lead refused (task #922), until they're seen on the build page.
    for a in s.refused ?? [] {
        out.append(Line(text: "The lead refused this Mac's ask to \(a.what) (\(clock(a.at))): \(a.refused ?? "no reason given") · Dismiss it on the build page", style: .plain))
    }
    // Every running job, slot by slot (task #846: a Mac runs up to six).
    let jobs = s.allJobs
    for (i, j) in jobs.enumerated() {
        out.append(Line(text: jobs.count > 1 ? "Slot \((j.slot ?? i) + 1): \(j.what)" : j.what, style: .plain))
        // How far the job says it is, and the time it has left at its pace.
        var bar: Line? = nil
        if let p = j.progress {
            bar = Line(text: progressText(p, paused: j.paused != nil, now: r.now), style: .bar, fraction: p.total > 0 ? min(1, p.done / p.total) : 0)
        }
        // Its parts, done, under way and to come, the bar under the one under way (unless the bar
        // only counts the parts, which the list shows).
        if let parts = j.parts, !parts.isEmpty, let cur = j.part {
            let partsBar = j.progress.map { $0.nouns.hasPrefix("parts") } ?? false
            for (i, p) in parts.enumerated() {
                let mark = i < cur ? "✓" : i == cur ? "▸" : "○"
                out.append(Line(text: "  \(mark) \(p)", style: i < cur ? .stepDone : i == cur ? .stepNow : .stepToDo))
                if i == cur, let b = bar, !partsBar { out.append(b) }
            }
        } else if let b = bar {
            out.append(b)
        }
        if let p = j.paused { out.append(Line(text: p, style: .small)) } else if j.pausing != nil { out.append(Line(text: "Stopping at its next safe point (what it's on is kept)", style: .small)) }
        if let w = j.waiting { out.append(Line(text: w.prefix(1).uppercased() + w.dropFirst(), style: .small)) }
        out.append(Line(text: "Running \(duration(r.now - j.started)) (since \(clock(j.started)))", style: .small))
        // The log's last lines, without the terminal's colour codes (one job alone: the menu stays short).
        guard jobs.count == 1 else { continue }
        let plain = (j.tail ?? "").replacingOccurrences(of: "\u{1B}\\[[0-9;]*[A-Za-z]", with: "", options: .regularExpression)
        for l in plain.split(separator: "\n").suffix(3) where !l.trimmingCharacters(in: .whitespaces).isEmpty {
            out.append(Line(text: tildes(String(l).trimmingCharacters(in: .whitespaces)), style: .mono))
        }
    }
    // The other Macs helping, each with every job.
    for h in freshHelpers(r) {
        if h.allJobs.isEmpty {
            out.append(Line(text: "\(h.host) is helping; nothing for it to build now", style: .small))
            continue
        }
        for j in h.allJobs {
            out.append(Line(text: "\(h.host): \(j.what)", style: .plain))
            if let p = j.progress {
                out.append(Line(text: progressText(p, paused: j.paused != nil, now: r.now), style: .bar, fraction: p.total > 0 ? min(1, p.done / p.total) : 0))
            }
            if let p = j.paused { out.append(Line(text: "\(h.host): \(p)", style: .small)) } else if j.pausing != nil { out.append(Line(text: "\(h.host): stopping at its next safe point", style: .small)) }
        }
    }
    // A Mac's loop stuck (task #899), this Mac's and (on the lead) each helper's.
    // (This Mac's only while it's heard from: an out-of-touch agent's said once, above.)
    let here = r.now - s.beat <= outOfTouch(r) ? [(s.host, loopStuck(age: s.loop_age_s, limit: s.loop_stuck_s, phase: s.loop_phase, beat: s.beat, now: r.now))] : []
    for (host, stuck) in here + freshHelpers(r).map({ ($0.host, loopStuck(age: $0.loop_age_s, limit: $0.loop_stuck_s, phase: $0.loop_phase, beat: $0.beat, now: r.now)) }) {
        if let t = stuck { out.append(Line(text: "\(host): \(t)", style: .plain)) }
    }
    let power = s.conditions.ac ? "Mains power" : "Battery\(s.conditions.battery.map { " \($0)%" } ?? "")"
    out.append(Line(text: "\(power) · NAS \(!s.conditions.nas ? "not reachable" : s.conditions.home == false ? "through Tailscale" : "reachable")", style: .small))
    out.append(Line(text: "\(s.host) (this Mac) · heard from \(duration(r.now - s.beat)) ago", style: .small))
    if let app = s.app { out.append(Line(text: "App \(app)", style: .small)) }
    // This Mac's build caches: what a clear would free, and the last clear.
    if let c = own?.caches { out.append(Line(text: cachesLine(c), style: .small)) }
    // The pool: who leads, each member (this Mac's agent's view).
    if let v = own?.pool?.lead { out += poolLines(v, now: r.now) }
    // The build to the end: each step done, under way, or to come.
    if let steps = s.checklist, !steps.isEmpty {
        let now = s.allJobs.map { String($0.id.split(separator: " ").first ?? "") }
        let finished = steps.filter(\.isDone).count
        out.append(Line(text: "", style: .separator))
        let legend = steps.contains { !$0.isDone && $0.shared != nil } ? "  ·  ⇄ any Mac can take part" : ""
        out.append(Line(text: "To the end: \(finished) of \(steps.count) stages done\(legend)", style: .header))
        for st in steps {
            if st.isDone {
                out.append(Line(text: "✓ \(st.what)", style: .stepDone))
                continue
            }
            let count = ": " + st.said.joined(separator: " · ")
            let running = now.contains { st.steps.contains($0) }
            let mark = st.shared != nil ? "  ⇄" : ""
            out.append(Line(text: "\(running ? "▸" : "○") \(st.what)\(count)\(mark)", style: running ? .stepNow : .stepToDo))
            // Its steps, each the same: done, under way or to come, its count in its noun.
            for it in st.items ?? [] {
                let here = now.contains(it.step)
                out.append(Line(text: "      \(it.isDone ? "✓" : here ? "▸" : "○") \(it.step): \(it.said)", style: .small))
            }
            // (Only some of its jobs: which.)
            if let sh = st.shared, sh != "all" { out.append(Line(text: "      ⇄ any Mac can take its \(sh)", style: .small)) }
            // Which Mac is on it or why it waits; then its jobs left, in order, by name.
            if let n = st.note { out.append(Line(text: "      \(n)", style: .small)) }
            let next = st.next ?? []
            for (i, j) in next.prefix(4).enumerated() {
                let when = i > 0 ? "then" : running ? "now" : "next"
                out.append(Line(text: "      \(when): \(j)", style: .small))
            }
            if next.count > 4 { out.append(Line(text: "      and \(next.count - 4) more", style: .small)) }
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
        // Whole regions (the checklist counts the z6 tiles, each once: regions share z6 tiles).
        let done = built.values.filter { $0.total > 0 && $0.built >= $0.total }.count
        out.append(Line(text: "", style: .separator))
        out.append(Line(text: "Regions built: \(done) of \(built.count)", style: .small))
        // Those still building: on the map as their tiles come, and
        // when all of it is (the forecast's).
        // Every z6 tile built but not yet recorded built (publishing's held regions): finishing.
        let held = Set((s.publishing?.regions ?? []).filter { r in r.kinds.contains { k in k != "new region" && k != "rebuilt" } }.map(\.id))
        for r in s.regions ?? [] {
            if let b = built[r.id], b.built < b.total {
                let all = s.forecast?.mapAt(r.id).map { "; all ≈ \(soon($0))" } ?? ""
                out.append(Line(text: "\(r.name): \(b.built) of \(b.total) tiles, on the map as they come\(all)", style: .small))
            } else if let b = built[r.id], held.contains(r.id) {
                out.append(Line(text: "\(r.name): finishing: \(b.built) of \(b.total) tiles, the last map tiles and layers being made", style: .small))
            }
        }
    }
    return out
}

/// This Mac's build caches in a line: what a clear would free, and the last clear.
func cachesLine(_ c: Caches) -> String {
    var t = "Build caches here: \(c.clearable.map(gb) ?? "not counted yet")"
    if let d = c.clearable_deep { t += " (\(gb(d)) deeply)" }
    if let f = c.cleared, f.why_not == nil { t += " · cleared \(clock(f.at)), \(gb(f.bytes)) freed" }
    if let f = c.cleared_deep, f.why_not == nil { t += " · cleared deeply \(clock(f.at)), \(gb(f.bytes)) freed" }
    return t
}

/// A repeating timer on the main run loop in its common modes: it fires while a menu is open too
/// (the default mode alone is suspended then, and what it held up all came at once when the menu
/// closed).
@discardableResult
func every(_ secs: TimeInterval, _ f: @escaping () -> Void) -> Timer {
    let t = Timer(timeInterval: secs, repeats: true) { _ in f() }
    RunLoop.main.add(t, forMode: .common)
    return t
}

/// Once, after `secs`, on the main run loop in its common modes. (Not a block on the main queue: a
/// menu opened from one would hold that queue, the polls' answers with it, until it closed.)
func after(_ secs: TimeInterval, _ f: @escaping () -> Void) {
    RunLoop.main.add(Timer(timeInterval: secs, repeats: false) { _ in f() }, forMode: .common)
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

/// A view whose subviews lay out from the top.
final class Flipped: NSView {
    override var isFlipped: Bool { true }
}

/// This Mac's agent's build page (a sandbox's own: Info.plist's ScenicPage), opened in the browser
/// by a held unit's Show on the build page.
let buildPage = info["ScenicPage"] as? String ?? "http://127.0.0.1:8090/"

/// This Mac's ask to its agent (crates/jobrt/src/control.rs), while it waits to be taken up: to
/// pause (true) or go on (false).
let askFile = "pause-request.json"
func pendingAsk() -> Bool? {
    guard let d = try? Data(contentsOf: home.appendingPathComponent("agent").appendingPathComponent(askFile)),
          let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return nil }
    return !(o["pause"] is NSNull || o["pause"] == nil)
}

/// The pool as this Mac's agent's pool last stepped it (`agent/pool/view.json`: crates/pipeline/
/// src/agent/poolside.rs PoolFile), written each of its steps, every 20 s or so whatever the agent's
/// loop is doing (agents from #197 on).
struct PoolFile: Decodable {
    let beat: Int
    let pool: OwnPool
}

/// This Mac's own agent's status (`Own`): the lead's or a helper's file, the fresher; its pool
/// part from the pool's own view when that's fresher (the status waits for the agent's loop).
func ownStatus() -> Own? {
    let dir = home.appendingPathComponent("agent")
    guard let own = ["status.json", "helper.json"].compactMap({ (try? Data(contentsOf: dir.appendingPathComponent($0))).flatMap { try? JSONDecoder().decode(Own.self, from: $0) } }).max(by: { $0.beat < $1.beat }) else { return nil }
    if let v = (try? Data(contentsOf: dir.appendingPathComponent("pool/view.json"))).flatMap({ try? JSONDecoder().decode(PoolFile.self, from: $0) }), v.beat > own.beat {
        return Own(host: own.host, app: own.app, beat: own.beat, caches: own.caches, pool: v.pool, hold: own.hold, out_of_touch_s: own.out_of_touch_s, memory: own.memory)
    }
    return own
}

/// This Mac's ask to its agent to clear its build caches (room.rs ClearRequest), while it waits or
/// is under way (taken up: renamed aside until it's answered).
let clearFile = "clear-request.json"
func clearAsked() -> Bool {
    [clearFile, "\(clearFile).taken"].contains { FileManager.default.fileExists(atPath: home.appendingPathComponent("agent").appendingPathComponent($0).path) }
}

/// "51.2 GB", or under a GB, "350 MB" (number and unit never split across lines).
func gb(_ b: Int) -> String {
    b >= 1 << 30 ? String(format: "%.1f\u{00A0}GB", Double(b) / Double(1 << 30)) : "\(b >> 20)\u{00A0}MB"
}

/// What a clear of that level keeps, in words (room.rs kept_words, the same).
func keptWords(deep: Bool) -> String {
    deep
        ? "Kept: the heritage clip while it can't be made again, answers the NAS hasn't all of (all of them while a job keeping them runs), what exists only here (files the NAS lacks, the z6 tiles' results not on the NAS yet), files a running job holds, and the map's offline copy."
        : "Kept: what's costly to make again (the heritage clip, the Wikidata and Wikipedia answers: a deep clear takes those too), what exists only here (files the NAS lacks, the z6 tiles' results not on the NAS yet), files a running job holds, and the map's offline copy."
}

/// The menu's items for this Mac's build caches (none without an agent here): Clear the build
/// caches and Clear caches deeply, each with what it would free now, enabled at any moment (why
/// not, in its tooltip: an earlier agent's job running, no NAS, its agent out of touch); while an
/// ask waits, that it's clearing.
func cachesItems(_ own: Own?, now: Int, asked: Bool) -> [(title: String, enabled: Bool, tip: String, deep: Bool)] {
    guard let own = own, let c = own.caches else { return [] }
    if asked { return [("Clearing the build caches… (asked; this Mac's agent does it now, jobs running keeping what they use)", false, "", false)] }
    return [false, true].map { deep in
        let title = (deep ? "Clear caches deeply" : "Clear the build caches") + (c.clearable(deep: deep).map { " (\(gb($0)))" } ?? "")
        if now - own.beat > (own.out_of_touch_s ?? outOfTouchDefault) { return (title, false, "This Mac's agent hasn't written its status since \(clock(own.beat)): is it running?", deep) }
        if let why = c.why_not { return (title, false, "Not now: \(why)", deep) }
        if let n = c.clearable(deep: deep), n < 50 << 20 { return (title, false, "They hold nothing now", deep) }
        let what = deep
            ? "Deletes what Clear the build caches does, and what's costly to make again: the heritage jobs' planet clip (an hour of osmium) and the Wikidata and Wikipedia answers the NAS has (copied back as the items and heritage jobs start)."
            : "Deletes this Mac's copies of what the NAS keeps (canopy squares, raw terrain tiles, base packs, copies of the NAS's files …), what queued jobs will read included: later jobs copy back from the NAS what they need."
        return (title, true, "\(what) Jobs running keep what they use. The map's offline copy stays.", deep)
    }
}

/// The ask's file as the menu bar writes it (crates/agentcore/src/agent/room.rs request_clear,
/// ClearRequest). Pinned on both sides by tools/status/test/clear-request-*.json (selfTest,
/// room.rs's tests).
func clearRequestJSON(by: String, at: Int, deep: Bool) -> Data {
    (try? JSONSerialization.data(withJSONObject: ["by": by, "at": at, "level": deep ? "deep" : "caches"] as [String: Any], options: [.sortedKeys])) ?? Data()
}

final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    lazy var item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    var reply: Reply?
    /// This Mac's agent's own status (its caches), read with each answer.
    var own: Own?
    var polling = false
    /// The app's build service and the window (service.swift).
    let build = BuildService()
    lazy var window = StatusWindow()
    /// Quitting goes ahead without asking: the build service stopped as asked, or the app replaced
    /// by an update (the build service left running).
    var mayQuit = false
    /// Whether this run manages the services (the app opened, not a --menu-proof run).
    var managing = false
    /// `--menu-proof`: its polls printed with the time, a menu held open between.
    var proof = false
    func applicationDidFinishLaunching(_ n: Notification) {
        if !proof { openApp() }
        // Any click on the icon (left, right, Control-) opens the menu, made afresh as it opens
        // (`menuNeedsUpdate`).
        item.menu = itemMenu
        show()
        poll()
        every(5) { [weak self] in self?.poll() }
        every(60) { [weak self] in self?.updateIfReplaced() }
        if proof { menuProof() }
    }

    /// `--menu-proof`: after a few seconds the menu opens and stays open 25 s (closed by a timer of
    /// its own), then it quits 12 s later; each poll printed with the time. With the timers in the
    /// common modes the polls go on every 5 s while the menu is open.
    func menuProof() {
        after(6) {
            print("\(stamp()) menu opens")
            after(25) { self.itemMenu.cancelTracking() }
            self.item.button?.performClick(nil)
            print("\(stamp()) menu closed")
            after(12) { exit(0) }
        }
    }

    // MARK: the app: its build service, quitting, updates (service.swift)

    /// The app opened: its menu, its build service started, the window (unless opened at login),
    /// and the app among the login items.
    func openApp() {
        // (Run from inside an installed version, as the old menu bar login item runs it before the
        // switch-over: the menu bar item alone, managing nothing, quitting for a newer version.)
        if insideInstalledVersion() {
            every(60) { quitIfReplaced() }
            return
        }
        let menu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        let name = Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "Scenic"
        appMenu.addItem(withTitle: "Show the \(name) window", action: #selector(showWindow), keyEquivalent: "0").target = self
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Hide \(name)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(withTitle: "Quit \(name)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        menu.addItem(appItem)
        let windowItem = NSMenuItem()
        let windowMenu = NSMenu(title: "Window")
        windowMenu.addItem(withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        windowItem.submenu = windowMenu
        menu.addItem(windowItem)
        NSApp.mainMenu = menu
        managing = true
        build.start()
        addLoginItem()
        updateWindow()
        // (Not at login, nor reopened after an update.)
        if !launchedAtLogin() && !CommandLine.arguments.contains("--no-window") { window.show() }
        updateIfReplaced()
    }

    /// The app made a login item (when it manages the build service and opens at login).
    func addLoginItem() {
        if build.blocked == nil, openAtLogin, SMAppService.mainApp.status != .enabled {
            do { try SMAppService.mainApp.register() } catch { NSLog("scenic: adding the app to the login items: \(error)") }
        }
    }

    /// Left alone at launch (the old login items still there), asked again each poll and as the
    /// menu opens: once they're gone, the build service registered and started and the app made a
    /// login item, as a fresh launch would.
    func recheckSwitchOver() {
        guard managing, build.recheck() else { return }
        addLoginItem()
        updateWindow()
    }

    /// Each poll: the build service kept running (or, quitting after its job, the app quits once
    /// it has stopped), the window.
    func tendServices() {
        // (Not in --menu-proof's runs: the services left alone.)
        guard managing else { return }
        recheckSwitchOver()
        build.tend()
        if build.quitting == "finish" && !build.state.running {
            mayQuit = true
            NSApp.terminate(nil)
            return
        }
        updateWindow()
    }

    func updateWindow() {
        let jobs = runningJobs(serviceRunning: build.state.running)
        var line: String? = jobs.isEmpty ? nil : jobsText(jobs)
        if build.quitting == "finish" { line = "Quitting once this job ends:\n" + (line ?? "") }
        window.update(build: build, job: line)
    }

    @objc func showWindow() {
        updateWindow()
        window.show()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { showWindow() }
        return true
    }

    /// A newer version installed (the agent put it under app/: jobrt::appupdate): this app replaced by it and opened
    /// again; the build service left running (its own update drain restarts it into that version).
    func updateIfReplaced() {
        guard build.quitting == nil, let new = newerApp() else { return }
        print("replacing this app with \(new.path)")
        if replaceSelf(with: new) {
            mayQuit = true
            NSApp.terminate(nil)
        }
    }

    /// Quitting (the menu bar item's menu; a logout, restart or shutdown): the build
    /// service stopped. No job running: at once. A job running: asked, "Finish this job first" (the
    /// app quits once the service has stopped) or "Stop now". A logout, restart or shutdown: "Stop
    /// now" without asking. On the lead the build waits for it (the lead is fixed, docs/pool.md §6).
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if mayQuit || !managing || build.blocked != nil { return .terminateNow }
        build.refresh()
        if !build.state.running { return .terminateNow }
        if quitForLogout() {
            build.askQuit(finish: false, why: ", logging out")
            return .terminateNow
        }
        let jobs = runningJobs(serviceRunning: true)
        if jobs.isEmpty {
            build.askQuit(finish: false)
            return .terminateNow
        }
        let a = NSAlert()
        a.messageText = jobs.count == 1 ? "A build job is running on this Mac" : "\(jobs.count) build jobs are running on this Mac"
        a.informativeText = jobsText(jobs) + "\n\nFinish this job first: nothing new is taken, and Scenic quits once it ends. Stop now: it's given back to the build, its work kept on this Mac's disk."
        a.addButton(withTitle: "Finish this job first")
        a.addButton(withTitle: "Stop now")
        a.addButton(withTitle: "Cancel")
        NSApp.activate()
        switch a.runModal() {
        case .alertFirstButtonReturn:
            build.askQuit(finish: true)
            updateWindow()
            show()
            return .terminateCancel
        case .alertSecondButtonReturn:
            build.askQuit(finish: false)
            return .terminateNow
        default:
            return .terminateCancel
        }
    }

    @objc func cancelQuit(_ sender: NSMenuItem) {
        build.cancelQuit()
        updateWindow()
        show()
    }

    @objc func stopNowAndQuit(_ sender: NSMenuItem) {
        build.askQuit(finish: false)
        mayQuit = true
        NSApp.terminate(nil)
    }

    @objc func quitApp(_ sender: NSMenuItem) {
        NSApp.terminate(nil)
    }

    func poll() {
        tendServices()
        if polling { return }
        polling = true
        DispatchQueue.global().async {
            let r = fetchReply()
            DispatchQueue.main.async {
                if self.proof { print("\(stamp()) poll: \(r.map { classify($0).1 } ?? "no answer")") }
                self.polling = false
                self.reply = r
                self.own = ownStatus()
                self.show()
            }
        }
    }

    // MARK: the icon and the menu

    /// A menu action whose ask to this Mac's agent couldn't be written (Accept, Cancel, Pause,
    /// Resume, Clear): said in place of the state (the icon a problem, its tooltip, the menu's
    /// first line) until an action's ask is written or a minute has passed.
    var failure: (said: String, at: Int)?

    func failed(_ what: String, _ error: Error) {
        failure = ("\(what): \(error.localizedDescription)", Int(Date().timeIntervalSince1970))
        NSLog("scenic-status: %@", failure!.said)
        show()
    }

    /// An action's ask written: any earlier failure said no more, and the answer read now.
    func asked() {
        failure = nil
        poll()
    }

    /// The state as shown: an action's failure while it's said, else the agent's.
    func shownState() -> (Kind, String) {
        if let f = failure, Int(Date().timeIntervalSince1970) - f.at < 60 { return (.problem, f.said) }
        failure = nil
        return state(reply, own: own)
    }

    func show() {
        let (kind, line) = shownState()
        if let b = item.button {
            let held = heldInputs(reply)
            b.image = iconImage(kind, held: !held.isEmpty, line: line)
            b.toolTip = held.isEmpty ? line : "\(line)\n" + held.map { "Held at the gate: \(inputLine($0))" }.joined(separator: "\n")
        }
    }

    /// The icon's menu, the item's for good: any click opens it, filled afresh just before
    /// (`menuNeedsUpdate`).
    lazy var itemMenu: NSMenu = {
        let m = NSMenu()
        m.autoenablesItems = false
        m.delegate = self
        return m
    }()

    func menuNeedsUpdate(_ m: NSMenu) {
        recheckSwitchOver()
        let (kind, line) = shownState()
        fill(m, kind, line)
    }

    /// The menu, `m` emptied and filled: the state in a line, this Mac's memory and build caches,
    /// then the controls.
    func fill(_ m: NSMenu, _ kind: Kind, _ line: String) {
        m.removeAllItems()
        var info = [Line(text: line, style: .title)]
        // (This Mac's agent running another app than this one: quitting and opening this one again
        // puts them back in step.)
        if let other = own?.app, let mine = bundleVersion(Bundle.main.bundleURL), other != mine {
            info.append(Line(text: "This Mac's agent runs app \(other), not this app's: quit and open Scenic again", style: .plain))
        }
        // (This Mac's memory against its floor, the brake first when it's on: task #932.)
        if let mem = own?.memory { info.append(Line(text: mem.said, style: mem.brake != nil ? .plain : .small)) }
        if let c = own?.caches { info.append(Line(text: cachesLine(c), style: .small)) }
        if managing && build.refused { info.append(Line(text: BuildService.refusedLine, style: .small)) }
        for l in info {
            let (font, color) = fontFor(l.style)
            // A line of information, not a button: it wraps rather than being cut short, and
            // neither highlights nor does anything when clicked.
            let it = NSMenuItem(title: l.text, action: nil, keyEquivalent: "")
            it.view = LineView(l.text, font: font, color: color, wrapAnywhere: false)
            m.addItem(it)
        }
        m.addItem(.separator())
        // The gate (docs/inputs.md §4.7): a line per held unit, with Accept (its warnings, after a
        // confirmation naming them) and the build page, where its banner says what holds it.
        let held = heldInputs(reply)
        for v in held {
            let it = NSMenuItem(title: inputLine(v), action: nil, keyEquivalent: "")
            let sub = NSMenu()
            let warns = (v.findings ?? []).filter { $0.level == "warning" }
            let acc = NSMenuItem(title: warns.isEmpty ? "Errors: fix or remove the files" : "Accept \(warns.count == 1 ? "the warning" : "\(warns.count) warnings")…", action: warns.isEmpty ? nil : #selector(acceptInputs), keyEquivalent: "")
            acc.target = self
            acc.isEnabled = !warns.isEmpty
            acc.representedObject = v.unit
            sub.addItem(acc)
            let show = NSMenuItem(title: "Show on the build page", action: #selector(openPage), keyEquivalent: "")
            show.target = self
            show.representedObject = buildPage + "#inputs"
            sub.addItem(show)
            it.submenu = sub
            m.addItem(it)
        }
        if !held.isEmpty { m.addItem(.separator()) }
        // The whole build paused (every Mac), or going on: an ask to this Mac's agent, which passes it
        // on to the lead (crates/jobrt/src/control.rs). Option: at once, frozen where it is.
        // (Paused as this Mac knows it: a member's, from its lead's line; the lead's, its own.)
        let pausedHere = buildPause(reply) != nil
        let asked = pendingAsk()
        if let a = asked {
            let it = NSMenuItem(title: a ? "Pausing… (asked; the lead takes it up within seconds)" : "Resuming… (asked)", action: nil, keyEquivalent: "")
            it.isEnabled = false
            m.addItem(it)
        }
        // (The opposite of what was asked, or of the state, is always there: an ask is never stuck.)
        if asked == true || (asked == nil && pausedHere) {
            let it = NSMenuItem(title: "Resume building", action: #selector(resumeBuild), keyEquivalent: "")
            it.target = self
            m.addItem(it)
        } else {
            let it = NSMenuItem(title: "Pause building", action: #selector(pauseBuild), keyEquivalent: "")
            it.target = self
            it.toolTip = "Every Mac's running job stops at its next safe point (a z3 tile, a z6 tile, a map tile), keeping what it did; nothing new starts until you resume"
            m.addItem(it)
            let now = NSMenuItem(title: "Pause building now", action: #selector(pauseBuildNow), keyEquivalent: "")
            now.target = self
            now.isAlternate = true
            now.keyEquivalentModifierMask = [.option]
            now.toolTip = "Every Mac's running job frozen where it is at once; it goes on from there when you resume"
            m.addItem(now)
        }
        // This Mac alone paused, or going on (`scenic pause --here`, task #224): its jobs stop at
        // their next safe point and are given back, it takes no new work; the rest of the pool goes
        // on.
        for h in holdItems(reply, own: own) {
            let it = NSMenuItem(title: h.title, action: h.held.map { $0 ? #selector(pauseHere) : #selector(resumeHere) }, keyEquivalent: "")
            it.target = self
            it.isEnabled = h.held != nil
            if !h.tip.isEmpty { it.toolTip = h.tip }
            m.addItem(it)
        }
        // A running job of this Mac's cancelled (`scenic job cancel`, crates/jobrt/src/control.rs
        // Cancel): stopped and given back, not failed, its work under way dropped. Option: held
        // until the lead runs a newer app.
        for j in runningJobs(serviceRunning: build.state.running) {
            guard let t = j.target else { continue }
            let it = NSMenuItem(title: "Cancel \(lowerFirst(j.what))", action: #selector(cancelJob), keyEquivalent: "")
            it.target = self
            it.representedObject = [j.step, t, "rerun"]
            it.toolTip = "Stopped now and given back to the build (not a failure), its work under way dropped; it runs again from the start"
            m.addItem(it)
            let alt = NSMenuItem(title: "Cancel \(lowerFirst(j.what)), rerun on a newer app", action: #selector(cancelJob), keyEquivalent: "")
            alt.target = self
            alt.representedObject = [j.step, t, "update"]
            alt.isAlternate = true
            alt.keyEquivalentModifierMask = [.option]
            alt.toolTip = "Stopped and given back as with Cancel, and held until the lead runs a newer app than this one"
            m.addItem(alt)
        }
        // This Mac's build caches, cleared on an ask to its agent (crates/agentcore/src/agent/
        // room.rs), which does it at once, to the item's level, and says what it freed and kept.
        for c in cachesItems(own, now: Int(Date().timeIntervalSince1970), asked: clearAsked()) {
            let it = NSMenuItem(title: c.title, action: c.enabled ? #selector(clearCaches) : nil, keyEquivalent: "")
            it.target = self
            it.isEnabled = c.enabled
            it.tag = c.deep ? 1 : 0
            if !c.tip.isEmpty { it.toolTip = c.tip }
            m.addItem(it)
        }
        m.addItem(.separator())
        // This Mac's map (docs/roles.md §8.3, 7a): its own server, started on demand where none is
        // installed (a laptop), so the map and its downloads work with the Mac mini away (away from
        // the NAS too, only what's downloaded: docs/roles.md §8.3).
        do {
            let it = NSMenuItem(title: mapStarting ? "Opening the map on this Mac…" : "Open the map on this Mac", action: mapStarting ? nil : #selector(openMapHere), keyEquivalent: "")
            it.target = self
            it.isEnabled = !mapStarting
            it.toolTip = "The map served by this Mac at \(mapHere.absoluteString): it works with the Mac mini away (away from the NAS too, with what this Mac downloaded); started if it isn't running, it quits 30 minutes after its last use"
            m.addItem(it)
        }
        if let log = reply?.log {
            let it = NSMenuItem(title: "Open the build log", action: #selector(openLog), keyEquivalent: "")
            it.target = self
            it.representedObject = log
            m.addItem(it)
        }
        // The window, and quitting.
        if managing {
            m.addItem(.separator())
            if build.blocked != nil {
                let it = NSMenuItem(title: build.said, action: nil, keyEquivalent: "")
                it.isEnabled = false
                m.addItem(it)
            }
            let win = NSMenuItem(title: "Show the Scenic window", action: #selector(showWindow), keyEquivalent: "")
            win.target = self
            m.addItem(win)
            if build.quitting == "finish" {
                let jobs = runningJobs(serviceRunning: build.state.running)
                let info = NSMenuItem(title: "Quitting once this job ends" + (jobs.first?.left.map { ", about \(duration($0)) left" } ?? ""), action: nil, keyEquivalent: "")
                info.isEnabled = false
                m.addItem(info)
                let cancel = NSMenuItem(title: "Don't quit", action: #selector(cancelQuit), keyEquivalent: "")
                cancel.target = self
                m.addItem(cancel)
                let now = NSMenuItem(title: "Stop now and quit", action: #selector(stopNowAndQuit), keyEquivalent: "")
                now.target = self
                m.addItem(now)
            } else {
                let quit = NSMenuItem(title: "Quit Scenic", action: #selector(quitApp), keyEquivalent: "")
                quit.target = self
                m.addItem(quit)
            }
        }
    }

    /// Accepts a held unit's warnings (the item's unit), once its owner says so, naming them: an ask
    /// to this Mac's agent (crates/inputs/src/inputs/mod.rs Ask, a file of its own in
    /// `inputs-asks/`), which writes the acceptances itself within seconds.
    @objc func acceptInputs(_ sender: NSMenuItem) {
        guard let unit = sender.representedObject as? String, let v = heldInputs(reply).first(where: { $0.unit == unit }) else { return }
        let warns = (v.findings ?? []).filter { $0.level == "warning" }
        guard !warns.isEmpty else { return }
        let alert = NSAlert()
        alert.messageText = "Accept \(warns.count == 1 ? "this warning" : "these \(warns.count) warnings") of \(unit)?"
        alert.informativeText = warns.map { "• \($0.message)" }.joined(separator: "\n") + "\n\nThe held change is taken in at the next check, and the map built with it. `scenic inputs unaccept` undoes it."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Accept")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate()
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "this Mac"
        let now = Int(Date().timeIntervalSince1970)
        let dir = home.appendingPathComponent("agent/inputs-asks")
        let file = String(format: "%020d-menu-%d.json", now, getpid())
        let (tmp, dst) = (dir.appendingPathComponent(file + ".menu.tmp"), dir.appendingPathComponent(file))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: ["unit": unit, "accept": warns.map(\.id), "by": "the menu bar on \(name)", "at": now] as [String: Any]).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            failed("Couldn't accept \(unit)'s warnings", error)
            return
        }
        asked()
    }

    /// Asks this Mac's agent to cancel a running job (its step, target, and "update" to hold it
    /// until a newer app), once its owner says so: an ask in a file of its own in the agent's cancel
    /// asks' folder (control.rs CANCEL_REQUESTS), written whole, which the agent takes up within
    /// seconds.
    @objc func cancelJob(_ sender: NSMenuItem) {
        guard let a = sender.representedObject as? [String], a.count == 3 else { return }
        let alert = NSAlert()
        alert.messageText = "Cancel the job building \(a[1])?"
        alert.informativeText = "It stops now and is given back to the build (not a failure), and its work under way is dropped: it runs again from the start" + (a[2] == "update" ? ", once the lead runs a newer app than this one." : ".")
        alert.addButton(withTitle: "Cancel the job")
        alert.addButton(withTitle: "Keep it running")
        NSApp.activate()
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "this Mac"
        let now = Date().timeIntervalSince1970
        let ask: [String: Any] = ["step": a[0], "target": a[1], "rerun_after_update": a[2] == "update", "by": "the menu bar on \(name)", "at": Int(now)]
        let dir = home.appendingPathComponent("agent/cancel-requests")
        let file = "\(Int(now * 1e9))-\(getpid())"
        let (tmp, dst) = (dir.appendingPathComponent(file + ".menu.tmp"), dir.appendingPathComponent(file + ".json"))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: ask).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            failed("Couldn't cancel the job", error)
            return
        }
        asked()
    }

    @objc func pauseBuild(_ sender: NSMenuItem) { ask("drain") }
    @objc func pauseBuildNow(_ sender: NSMenuItem) { ask("freeze") }
    @objc func resumeBuild(_ sender: NSMenuItem) { ask(nil) }
    @objc func pauseHere(_ sender: NSMenuItem) { askHold(true) }
    @objc func resumeHere(_ sender: NSMenuItem) { askHold(false) }

    /// Asks this Mac's agent to pause this Mac alone, or let it go on: its ask file
    /// (crates/jobrt/src/control.rs HoldRequest), written whole, taken up within seconds.
    func askHold(_ held: Bool) {
        let now = Int(Date().timeIntervalSince1970)
        let name = SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "this Mac"
        let dir = home.appendingPathComponent("agent")
        let (tmp, dst) = (dir.appendingPathComponent("hold-request.json.menu.tmp"), dir.appendingPathComponent("hold-request.json"))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try holdRequestJSON(held, by: "the menu bar on \(name)", at: now).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            failed("Couldn't ask this Mac to \(held ? "pause" : "go on")", error)
            return
        }
        asked()
    }

    /// Asks this Mac's agent to pause the build (`mode`: "drain" or "freeze") or let it go on (nil):
    /// its ask file (crates/jobrt/src/control.rs Request), written whole, which the agent takes up
    /// within seconds and passes on to the lead.
    func ask(_ mode: String?) {
        let now = Int(Date().timeIntervalSince1970)
        // (The Mac's name from its settings: no network lookup to wait on.)
        let name = SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "this Mac"
        let who = "the menu bar on \(name)"
        let pause: Any = mode.map { ["mode": $0, "by": who, "at": now] as [String: Any] } ?? NSNull()
        let dir = home.appendingPathComponent("agent")
        let (tmp, dst) = (dir.appendingPathComponent("pause-request.json.menu.tmp"), dir.appendingPathComponent(askFile))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: ["pause": pause, "at": now]).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            failed("Couldn't ask the build to \(mode == nil ? "go on" : "pause")", error)
            return
        }
        asked()
    }

    /// Asks this Mac's agent to clear its build caches to the item's level (its tag: 1 deeply),
    /// once its owner says so (what goes, how each comes back, and what stays): its ask file
    /// (room.rs ClearRequest), written whole, which the agent takes up within seconds; signed with
    /// the name the agent goes by (its Mac's local host name).
    @objc func clearCaches(_ sender: NSMenuItem) {
        guard let c = own?.caches else { return }
        let deep = sender.tag == 1
        let alert = NSAlert()
        alert.messageText = "Clear this Mac's build caches\(deep ? " deeply" : "") (\(gb(c.clearable(deep: deep) ?? 0)))?"
        let each = (c.each ?? []).filter { deep || $0.deep != true }.map { "• \($0.what) \(gb($0.bytes)): \($0.back)" }.joined(separator: "\n")
        alert.informativeText = "\(each)\n\n\(keptWords(deep: deep))"
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Clear")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate()
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = SCDynamicStoreCopyLocalHostName(nil) as String? ?? "this Mac"
        let dir = home.appendingPathComponent("agent")
        let (tmp, dst) = (dir.appendingPathComponent("\(clearFile).menu.tmp"), dir.appendingPathComponent(clearFile))
        do {
            try clearRequestJSON(by: "the menu bar on \(name)", at: Int(Date().timeIntervalSince1970), deep: deep).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            failed("Couldn't ask to clear the build's caches", error)
            return
        }
        asked()
    }

    /// Whether this Mac's map is being started (`openMapHere`).
    var mapStarting = false

    /// Opens this Mac's map in the browser, its server started first when it doesn't answer
    /// (`startMapHere`); what went wrong said as a failed action's is.
    @objc func openMapHere(_ sender: NSMenuItem) {
        guard !mapStarting else { return }
        mapStarting = true
        DispatchQueue.global().async {
            let why = startMapHere()
            DispatchQueue.main.async {
                self.mapStarting = false
                if let why {
                    self.failed("Couldn't open the map on this Mac", NSError(domain: "scenic", code: 1, userInfo: [NSLocalizedDescriptionKey: why]))
                } else {
                    NSWorkspace.shared.open(mapHere)
                }
            }
        }
    }

    @objc func openLog(_ sender: NSMenuItem) {
        if let p = sender.representedObject as? String { NSWorkspace.shared.open(URL(fileURLWithPath: p)) }
    }

    @objc func openPage(_ sender: NSMenuItem) {
        guard let p = sender.representedObject as? String, let u = URL(string: p) else { return }
        NSWorkspace.shared.open(u)
    }

}

// MARK: the map on this Mac (docs/roles.md §8.3, 7a)

/// This Mac's map server's address (coordwire::net::DEFAULT_PORT).
let mapHere = URL(string: "http://localhost:7311/")!

/// The server's command line for this Mac's map on demand, from the installed app as
/// tools/app/install.sh's run file names it, with `--on-demand` (crates/server: this Mac's
/// loopback alone, never retired, quitting 30 min after its last use).
func mapServerCommand(_ home: URL) -> [String] {
    let app = home.appendingPathComponent("app/current")
    return [app.appendingPathComponent("server").path, "--web", app.appendingPathComponent("web").path,
            "--fonts", app.appendingPathComponent("fonts").path, "--on-demand"]
}

/// Whether this Mac's map server answers (`/api/ping`) within `timeout` seconds. Never on the main
/// thread.
func mapAnswers(timeout: TimeInterval = 2) -> Bool {
    let req = URLRequest(url: mapHere.appendingPathComponent("api/ping"), cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: timeout)
    let done = DispatchSemaphore(value: 0)
    var ok = false
    URLSession.shared.dataTask(with: req) { _, r, _ in
        ok = (r as? HTTPURLResponse)?.statusCode == 200
        done.signal()
    }.resume()
    done.wait()
    return ok
}

/// This Mac's map answering: at once when its server runs (the installed one on the map's Mac, or
/// one started before); else, where no server is installed (a laptop: tools/app/install.sh installs
/// it on the map's Mac alone), its server started on demand (`mapServerCommand`, its output
/// appended to ~/Library/Logs/scenic/server.log), waited for up to two minutes (it reads its map at
/// its start). nil once it answers, else why not. Never on the main thread.
func startMapHere() -> String? {
    if mapAnswers() { return nil }
    let user = FileManager.default.homeDirectoryForCurrentUser
    let log = user.appendingPathComponent("Library/Logs/scenic/server.log")
    let installed = FileManager.default.fileExists(atPath: user.appendingPathComponent("Library/LaunchAgents/local.scenic.server.plist").path)
    var p: Process?
    if !installed {
        let cmd = mapServerCommand(home)
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: cmd[0])
        proc.arguments = Array(cmd.dropFirst())
        try? FileManager.default.createDirectory(at: log.deletingLastPathComponent(), withIntermediateDirectories: true)
        if !FileManager.default.fileExists(atPath: log.path) { FileManager.default.createFile(atPath: log.path, contents: nil) }
        guard let out = FileHandle(forWritingAtPath: log.path) else { return "can't write \(log.path)" }
        out.seekToEndOfFile()
        proc.standardOutput = out
        proc.standardError = out
        proc.standardInput = FileHandle.nullDevice
        do { try proc.run() } catch { return "\(cmd[0]): \(error.localizedDescription)" }
        p = proc
    }
    let end = Date().addingTimeInterval(120)
    while Date() < end {
        if mapAnswers() { return nil }
        if let p, !p.isRunning { return "its server ended (\(log.path))" }
        Thread.sleep(forTimeInterval: 0.5)
    }
    return installed ? "this Mac's map service doesn't answer" : "no answer within two minutes (\(log.path))"
}

/// "17:04:05.123", for --menu-proof's log.
func stamp() -> String {
    let f = DateFormatter()
    f.dateFormat = "HH:mm:ss.SSS"
    return f.string(from: Date())
}

let usage = """
usage: scenic-status [option]
  (none)                     the app: the menu bar item, the build service and its window
  --no-window                the app, its window not opened (an update's reopen)
  --plan                     whether opening it would manage the build service, or why not
  --services                 the build service's state
  --unregister               the build service and the app off the login items
  --print                    the icon, the status's lines and the menu's items now, as text
  --replay a.json b.json …   the state, badge and caches' items a sequence of answers would show
                             (each file `scenic menu`'s answer, with this Mac's agent's own status
                             beside it as "own", and "clear_asked" while an ask to clear its caches
                             waits)
  --render lines.png         the status's lines drawn as they lay out (dark), for checking
  --menu-proof               runs, its polls printed with the time, a menu held
                             open 25 s between: they go on while it's open
  --reopen <pid> <app>       an update's helper: opens <app> once <pid> has ended
  --test <dir>               its self-tests: arguments, the reopen helper, launchd's state read from
                             captured launchctl output (tools/status/test)
  --help, -h                 this
"""

/// The options and how many values each takes (-1: all the rest, at least one).
let options: [String: Int] = [
    "--no-window": 0, "--plan": 0, "--services": 0, "--unregister": 0, "--print": 0, "--replay": -1,
    "--render": 1, "--menu-proof": 0, "--reopen": 2, "--test": 1,
]

/// The options that run something other than the app: one at a time, and not with the app's own
/// (--no-window, --menu-proof).
let modes: Set<String> = ["--plan", "--services", "--unregister", "--print", "--replay", "--render", "--reopen", "--test"]

/// What's wrong with the arguments (after the program's name): nil when they're fine, "" for
/// --help/-h, else the problem. LaunchServices' own (-psn_…) are passed over. As the Rust
/// programs' (clap, docs/building.md "Command lines"): help anywhere wins; an unknown option, one
/// given twice, or two that can't go together is refused.
func argsProblem(_ a: [String]) -> String? {
    if a.contains("--help") || a.contains("-h") { return "" }
    var seen: [String] = []
    var i = 0
    while i < a.count {
        let o = a[i]
        if o.hasPrefix("-psn_") { i += 1; continue }
        guard let n = options[o] else { return "unknown argument \(o)" }
        if seen.contains(o) { return "\(o) given twice" }
        seen.append(o)
        let need = n < 0 ? 1 : n
        let values = a[(i + 1)...].prefix { !$0.hasPrefix("--") }
        if values.count < need { return "\(o) needs \(need == 1 ? "a value" : "\(need) values")" }
        i += 1 + (n < 0 ? values.count : n)
    }
    if let m = seen.first(where: { modes.contains($0) }), let other = seen.first(where: { $0 != m }) {
        return "\(m) and \(other) can't go together"
    }
    return nil
}

let args = CommandLine.arguments
if let p = argsProblem(Array(args.dropFirst())) {
    if p.isEmpty {
        print(usage, terminator: "")
        exit(0)
    }
    FileHandle.standardError.write(Data("scenic-status: \(p)\n\(usage)".utf8))
    exit(2)
}
if let i = args.firstIndex(of: "--reopen") {
    // The helper an update starts: the app opened again once the old process ended (service.swift).
    reopenHelper(args[i + 1], args[i + 2])
} else if let i = args.firstIndex(of: "--test") {
    selfTest(args[i + 1])
} else if args.contains("--unregister") {
    // The build service and the app taken off the login items (the service stopped and unloaded):
    // a sandbox's cleanup, or before the app is removed.
    for (what, s) in [("the build service", SMAppService.agent(plistName: "\(buildLabel).plist")), ("the app", SMAppService.mainApp)] {
        do {
            try s.unregister()
            print("unregistered \(what)")
        } catch {
            print("\(what): \(error.localizedDescription)")
        }
    }
} else if args.contains("--plan") {
    // What opening this copy would manage: nothing from inside an installed version or before the
    // switch-over (a check of the transition guard).
    print(notManaging().map { "manages nothing: \($0)" } ?? "manages the build service (\(buildLabel)) and the login items")
} else if args.contains("--services") {
    // The build service's state, as the app sees it.
    let b = BuildService()
    b.refresh()
    print("\(b.said) [\(b.service.status.rawValue)]")
} else if args.contains("--print") {
    // The status now, as the icon and menu would show it.
    let r = fetchReply()
    let (kind, line) = state(r, own: ownStatus())
    print("icon: \(kind.symbol)\(heldInputs(r).isEmpty ? "" : " with a warning badge")")
    for v in heldInputs(r) { print("item: \(inputLine(v))") }
    let own = ownStatus()
    for l in lines(r, line, own: own) {
        let bar = l.style == .bar ? "[" + String(repeating: "█", count: Int(l.fraction * 20)) + String(repeating: "░", count: 20 - Int(l.fraction * 20)) + "] " : ""
        print(l.style == .separator ? "────" : (l.style == .title ? "" : "  ") + bar + l.text)
    }
    for h in holdItems(r, own: own) { print("item: \(h.title)\(h.held == nil ? " (disabled)" : "")") }
    for c in cachesItems(own, now: Int(Date().timeIntervalSince1970), asked: clearAsked()) {
        print("item: \(c.title)\(c.enabled || c.tip.isEmpty ? "" : " (disabled: \(c.tip))")")
    }
} else if let i = args.firstIndex(of: "--render") {
    // The menu bar's icon and the menu's lines as views, stacked as the menu stacks them, drawn into a PNG.
    let r = fetchReply()
    _ = NSApplication.shared
    let (kind, line) = state(r, own: ownStatus())
    // (The menu bar's icon first, as the bar draws it: a template image, light on the dark bar.)
    let bar = NSImageView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: 24))
    bar.image = iconImage(kind, held: !heldInputs(r).isEmpty, line: line)
    bar.contentTintColor = .white
    bar.imageAlignment = .alignLeft
    let views: [NSView] = [bar] + lines(r, line, own: ownStatus()).map { l in
        l.style == .separator ? NSView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: 11)) : l.style == .bar ? BarView(l.text, fraction: l.fraction) : LineView(l.text, font: fontFor(l.style).0, color: fontFor(l.style).1, wrapAnywhere: l.style == .mono)
    } + holdItems(r, own: ownStatus()).map { hi in LineView(hi.title, font: .menuFont(ofSize: 0), color: hi.held != nil ? .labelColor : .tertiaryLabelColor, wrapAnywhere: false) }
        + (r?.log != nil ? ["Open the build log"] : []).map { LineView($0, font: .menuFont(ofSize: 0), color: .labelColor, wrapAnywhere: false) }
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
} else if let i = args.firstIndex(of: "--replay") {
    // The state and items a sequence of answers would show.
    let d = AppDelegate()
    for f in args[(i + 1)...] {
        let data = (try? Data(contentsOf: URL(fileURLWithPath: f))) ?? Data()
        d.reply = try? JSONDecoder().decode(Reply.self, from: data)
        let beside = try? JSONDecoder().decode(Replay.self, from: data)
        d.own = beside?.own
        print("\(f): \(state(d.reply, own: d.own).1)\(heldInputs(d.reply).isEmpty ? "" : " (badged)")")
        for v in heldInputs(d.reply) { print("  item: \(inputLine(v))") }
        for c in cachesItems(d.own, now: d.reply?.now ?? Int(Date().timeIntervalSince1970), asked: beside?.clear_asked ?? false) {
            print("  item: \(c.title)\(c.enabled || c.tip.isEmpty ? "" : " (disabled: \(c.tip))")")
            // (What its confirmation lists.)
            for g in ((c.enabled ? d.own?.caches?.each : nil) ?? []).filter({ c.deep || $0.deep != true }) { print("    \(g.what) \(gb(g.bytes)): \(g.back)") }
        }
    }
} else {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    delegate.proof = args.contains("--menu-proof")
    app.delegate = delegate
    // (Never in the Dock (Info.plist's LSUIElement too): the menu bar item and the app's window.)
    app.setActivationPolicy(.accessory)
    app.run()
}
