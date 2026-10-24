// Scenic's menu bar item: the build agent's state at a glance (docs/building.md §2, Status). An icon for
// the state (building, paused, waiting, nothing to do, a problem, the lead out of touch); a
// click opens the panel, the build page in a popover (`Panel`: this Mac's own agent's `/work/?view`, a viewer
// only), and a right-click, or the panel's "⋯", the menu: the state in a line, this Mac's downloads
// and build caches, and the controls below; and a notification for every change. Its timers run in
// the run loop's common modes, so the polls, the icon and the notifications go on while a menu is
// open. It asks the map's server on this Mac
// (`/api/build`), which answers with this Mac's own agent's status when the agent runs here and
// with the heartbeat the agent copies to the NAS otherwise. This Mac's own agent's status, read
// from its file, says what its build caches hold, whether they can be cleared now,
// and the last clear (crates/pipeline/src/agent/room.rs): Clear the Build's Caches asks
// that agent to clear them. Disk Room shows the disk's free space and the owner's room target, and
// sets it (a few presets, or Off): the free space that agent keeps, freeing its caches to it; its
// title says the free space, what's left of it for jobs and what keeps the rest. Memory shows and
// sets the memory the build's jobs get here together (crates/pipeline/src/agent/buildmem.rs: a few
// presets, or the memory guard's limit alone). With
// the pool on (docs/pool.md §11), its agent's view of the pool: who leads, each Mac and its state,
// and the lead's items: Hand the Build To ▸ on the lead, Make This Mac Lead and Take Over the
// Build… (confirming) on another Mac; each an ask to that agent
// (`lead-request.json`), which checks it as its driver would.
//
// It's the menu bar item of Scenic.app, the app opened from Launchpad, Spotlight or /Applications
// (and at login), never in the Dock: opening it starts this Mac's build service, quitting it stops that service, and
// its window starts and stops the map server, a service of its own (service.swift).
//
//   tools/status/bundle.sh <folder> [sandbox options]   builds Scenic.app (both files, its launcher
//                                                        and its build service's launchd plist)
//   scenic-status --help                    its arguments (`usage` below); an unknown one is refused
//                                           (exit 2), never the app started
// SCENIC_STATUS_SERVER overrides the server (http://127.0.0.1:7311; else Info.plist's ScenicServer),
// SCENIC_HOME the app folder (else Info.plist's ScenicHome), SCENIC_STATUS_APPEARANCE (light,
// dark) the system's appearance.

import AppKit
import CoreServices
import SystemConfiguration
import UserNotifications
import ServiceManagement
import WebKit

let server = URL(string: ProcessInfo.processInfo.environment["SCENIC_STATUS_SERVER"] ?? info["ScenicServer"] as? String ?? "http://127.0.0.1:7311")!
let home = (ProcessInfo.processInfo.environment["SCENIC_HOME"] ?? info["ScenicHome"] as? String).map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/scenic")
/// Without a heartbeat for this long, the lead counts as out of touch (asleep, off, away, or
/// its agent stuck): its agent writes one at least every two minutes.
let outOfTouch = 6 * 60

// What /api/build answers (the agent's status, crates/pipeline/src/agent/mod.rs Status).
struct Reply: Decodable {
    let status: Status?
    let local: Bool
    let now: Int
    let log: String?
    /// What this Mac has downloaded for offline use (servers from 2026-10-08 on).
    let offline: Offline?
    /// This Mac paused alone (crates/pipeline/src/control.rs Hold, task #224), and its ask while
    /// the agent hasn't taken it up (servers from #224 on).
    let held_here: HeldInfo?
    let hold_asked: Bool?
}

/// One Mac paused alone (crates/pipeline/src/control.rs Hold): who asked, since when.
struct HeldInfo: Decodable {
    let by: String
    let at: Int
}

/// This Mac's downloads (crates/server/src/downloads.rs `summary`).
struct Offline: Decodable {
    let world: Bool
    /// (Absent from a server of before: none counted, the menu still drawn.)
    let regions: Int?
    let bytes: Int
    let here: Int
    /// The NAS is reachable from this Mac.
    let nas: Bool
}

/// The menu's line on what this Mac can show without the NAS.
func offlineText(_ o: Offline) -> String {
    let n = o.regions ?? 0
    let regions = "\(n) region\(n == 1 ? "" : "s")"
    if !o.world && n == 0 {
        return o.nas ? "Nothing downloaded: the map needs the NAS to show anything (Settings → Regions)" : "Away from the NAS with nothing downloaded: the map can't show anything"
    }
    var t: String
    if !o.world {
        t = "Downloaded: \(regions), not the World: away from the NAS, zoomed out, the map shows nothing"
    } else {
        t = "Downloaded: the World, zoomed out"
        if n > 0 { t += " and \(regions)" }
    }
    if o.here < o.bytes { t += o.nas ? " · \(min(99, o.here * 100 / max(1, o.bytes))) % here" : " · \(min(99, o.here * 100 / max(1, o.bytes))) % here, away from the NAS" }
    return t
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
    let checklist: [Step]?
    /// Other Macs helping, building tiles (agents from 2026-10-04 on).
    let helpers: [Helper]?
    /// Every worker the coordinator heard from lately: helpers and web pages (agents from 2026-10-05 on).
    let workers: [Worker]?
    /// The build's pause, while it's paused (agents from 2026-10-05 on).
    let pause: PauseInfo?
    /// The regions (their names), and when the build will be done and the map next updated (the
    /// forecast: crates/pipeline/src/agent/forecast.rs; agents from 2026-10-05 on).
    let regions: [Recipe]?
    let forecast: Forecast?

    /// Its running jobs, every slot's in order (`jobs`, else the old fields).
    var allJobs: [Job] { jobs ?? ([job, beside].compactMap { $0 } + (more ?? [])) }
    /// Its first running job, and its second.
    var firstJob: Job? { allJobs.first }
    var secondJob: Job? { allJobs.dropFirst().first }
    /// The gate's units (docs/inputs.md §4.7; agents from #134 on): a held one badges the icon.
    let inputs: [InputUnit]?
}

/// A gate unit (crates/pipeline/src/inputs/view.rs InputView).
struct InputUnit: Decodable {
    let unit: String
    /// "ok", "checking" or "held" (held while it's checked again too: `checking`).
    let state: String
    let checking: Bool?
    let held: [String]?
    let findings: [InputFinding]?
}

/// A finding holding a gate unit's change (crates/pipeline/src/inputs/mod.rs Finding).
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
    let beat: Int
    let caches: Caches?
    /// This Mac in the pool, while it's on (crates/pipeline/src/agent/mod.rs PoolView).
    let pool: OwnPool?
}

/// This Mac in the pool: its member, and the pool as the controls show it (docs/pool.md §10, §11;
/// crates/pipeline/src/agent/lead.rs View; agents from 2026-10-08 on).
struct OwnPool: Decodable {
    let member: String
    let lead: PoolView?
}

struct PoolView: Decodable {
    let at: Int
    let term: Int
    let lead: LeadOf?
    let leading: Bool
    let members: [PoolMember]
    let takeover: TakeoverNeeds?
    let no_lead: String?
    /// No lead for an hour or more: since when, why, and what to do (agents from #195 on).
    let alert: String?
    let handing: Handing?
    /// Who can't reach whom, each with why (agents from #198 on: docs/pool.md §6.8).
    let unreached: [String]?
    let asked: LeadAsked?
    let change: LeadChange?
}

struct LeadOf: Decodable {
    let term: Int
    let member: String
    let host: String
    let app: String
    let since: Int
    let how: String
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
    let can_lead: Bool
    let why_not: String?
}

struct TakeoverNeeds: Decodable {
    let refused: String?
    let force: String?
    let downgrade: String?
}

struct Handing: Decodable {
    let to: String
    let host: String
    let term: Int
    let stage: String
    let since: Int
}

struct LeadAsked: Decodable {
    let by: String
    let at: Int
    let since: Int
    let state: String
    let said: String
}

struct LeadChange: Decodable {
    let at: Int
    let said: String
}

/// What a lead item does when chosen.
enum LeadAction {
    /// Hand the lead to a member (its id; its host; whether it's away, to warn).
    case give(String, String, Bool)
    /// This Mac takes the lead over, confirming first: since when the lead's been gone, and the
    /// owner's force and downgrade, each with why, when they're needed.
    case take(String, String?, String?)
}

/// A lead item of the menu (docs/pool.md §11): its title, whether it can be chosen and why not, what
/// it does, and its submenu's items.
struct LeadItem {
    let title: String
    var enabled = true
    var tip = ""
    var action: LeadAction? = nil
    var children: [LeadItem] = []
}

/// The pool's items for this Mac's menu: on the lead, "Hand the Build To ▸" its members, each with
/// its state, those that can't lead greyed with why; on another member, "Make This Mac Lead" and,
/// with no lead in touch, "Take Over the Build…"; an ask under way.
func leadItems(_ v: PoolView, me: String) -> [LeadItem] {
    var out: [LeadItem] = []
    if let a = v.asked, a.state == "passed" || a.state == "going" {
        out.append(LeadItem(title: "\(a.said)…", enabled: false))
    }
    if v.leading {
        let others = v.members.filter { $0.me != true }
        var it = LeadItem(title: "Hand the Build To", enabled: !others.isEmpty && v.handing == nil, tip: v.handing.map { "A handover to \($0.host) is under way" } ?? (others.isEmpty ? "No other Mac in the pool" : ""))
        it.children = others.map { m in
            LeadItem(title: "\(m.host) — \(m.state)", enabled: m.can_lead, tip: m.can_lead ? ((m.away ?? false) ? "Away from home: the build's duties run slowly over Tailscale" : "") : (m.why_not ?? ""), action: .give(m.member, m.host, m.away ?? false))
        }
        out.append(it)
    } else {
        let mine = v.members.first { $0.me == true }
        if v.no_lead == nil {
            out.append(LeadItem(title: "Make This Mac Lead", enabled: mine?.can_lead ?? false, tip: (mine?.can_lead ?? false) ? "Asks \(v.lead?.host ?? "the lead") to hand the build to this Mac" : (mine?.why_not ?? "not in the pool yet"), action: .give(me, mine?.host ?? "this Mac", false)))
        }
        if let why = v.no_lead, let t = v.takeover {
            out.append(LeadItem(title: "Take Over the Build…", enabled: t.refused == nil, tip: t.refused ?? why, action: .take(why, t.force, t.downgrade)))
        }
    }
    return out
}

/// The pool's lines for the menu: who leads, a handover under way, each member and its state, the
/// last change of lead, the ask's end.
func poolLines(_ v: PoolView, now: Int) -> [Line] {
    var out = [Line(text: "", style: .separator)]
    if let why = v.no_lead {
        out.append(Line(text: "The pool · term \(v.term) · no lead: \(why)", style: .header))
        if let a = v.alert { out.append(Line(text: a, style: .plain)) }
    } else if let l = v.lead {
        out.append(Line(text: "The pool · \(l.host) leads term \(l.term)\(v.leading ? " (this Mac)" : "") since \(clock(l.since))", style: .header))
    }
    if let h = v.handing {
        out.append(Line(text: "Handing over to \(h.host): \(h.stage) since \(clock(h.since))", style: .plain))
    }
    for m in v.members {
        let who = m.me == true ? "\(m.host) (this Mac)" : m.host
        let lead = m.leads == true ? " · leads" : ""
        let heard = m.out_of_touch == true && m.beat > 0 ? ", last heard \(duration(now - m.beat)) ago" : ""
        let why = m.leads != true && !m.can_lead ? " · can't lead: \(m.why_not ?? "")" : ""
        out.append(Line(text: "\(who)\(lead): \(m.state)\(heard)\(why)", style: .small))
    }
    for u in v.unreached ?? [] {
        out.append(Line(text: u, style: .plain))
    }
    if !(v.unreached ?? []).isEmpty {
        out.append(Line(text: "On the Mac named: check it is awake, on the network and its agent running; then allow Scenic in System Settings › Network › Firewall (Options…), or turn the firewall off", style: .small))
    }
    if let a = v.asked, ["done", "failed", "refused"].contains(a.state) {
        out.append(Line(text: "\(a.state == "done" ? "Done" : a.state == "refused" ? "Refused" : "Came to nothing"): \(a.said) (\(clock(a.since)))", style: .small))
    }
    if let c = v.change, now - c.at < 24 * 3600 {
        out.append(Line(text: "\(c.said) (\(clock(c.at)))", style: .small))
    }
    return out
}

/// This Mac's build caches (crates/pipeline/src/agent/room.rs Caches): what a clear would free
/// (bytes, and cache by cache with how each comes back), why they can't be cleared now (none: they
/// can), and the last clear done and the last ask declined.
struct Caches: Decodable {
    let clearable: Int?
    let each: [Gone]?
    let why_not: String?
    let cleared: Freed?
    let declined: Freed?
    let room: RoomView?
}

/// The owner's disk room target as the agent keeps it (room.rs RoomView): the target, the disk's
/// free space as its status was written, its last freeing toward it, and why the disk is short of
/// it and stays so.
struct RoomView: Decodable {
    let target: RoomTarget?
    let free: Int
    /// The disk its jobs have reserved (MB; nil from an older agent).
    let reserved_mb: Int?
    let toward: Freed?
    let short: String?
}

/// The target (room.rs Target): bytes to keep free, who set it and when.
struct RoomTarget: Decodable {
    /// (A Double: a hand-edited file's past Int's range doesn't drop the whole status.)
    let bytes: Double
    let by: String
    let at: Int

    /// The bytes, at most the agent's largest target (room.rs MAX_TARGET, 1 PB).
    var size: Int { Int(min(max(bytes, 0), Double(1 << 50))) }
}

/// A cache a clear would empty (room.rs Gone): its name in words, its bytes, how it comes back.
struct Gone: Decodable {
    let what: String
    let bytes: Int
    let back: String
}

/// What a clear did (room.rs Freed): when, the bytes freed by cache, what stays, and for
/// a clear asked for when it couldn't be, why not.
struct Freed: Decodable {
    let at: Int
    let by: String?
    let freed: [String: Int]?
    let left: Int?
    let why_not: String?

    var bytes: Int { (freed ?? [:]).values.reduce(0, +) }
}

struct Recipe: Decodable {
    let id: String
    let name: String
}

/// The forecast's part the menu shows: when it'll all be done (and the range), and when each region
/// is all on the map as it's built now (its `map_at`: its last z6 tile, its map tiles drawn, a catalog
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

/// The build's pause (crates/pipeline/src/control.rs Pause): at the next safe point ("drain") or
/// at once ("freeze"), who asked, since when.
struct PauseInfo: Decodable {
    let mode: String
    let by: String
    let at: Int
}

/// A worker, as the lead's coordinator knows it (docs/workers.md).
struct Worker: Decodable {
    let name: String
    let label: String
    let kind: String
    let what: String
    let done: Int
    let bad: Bool
}

/// A helper on another Mac, as the main agent last read its status (`here`: this Mac's own, fresh).
struct Helper: Decodable {
    let host: String
    let beat: Int
    /// Its running jobs (a newer agent's), else its first slot's (`job`): `firstJob`.
    let jobs: [Job]?
    let job: Job?
    var firstJob: Job? { jobs?.first ?? job }
    let pause: PauseInfo?
    let here: Bool?
}

/// A stage of the build to the end (docs/pipeline.md §4; agents before it: a group of steps): its
/// targets done of total (total unknown until an earlier step makes it) in its noun (`unit`), or
/// how many are left; its steps' lines (`items`, agents from 2026-10-17 on); its jobs left by name
/// (`next`, the first the one under way), and why it waits or which Mac is on it (`note`).
struct Step: Decodable {
    let what: String
    let steps: [String]
    let done: Int?
    let total: Int?
    let left: Int?
    let unit: String?
    let items: [Item]?
    let next: [String]?
    let note: String?
    /// Which of its jobs a helper may do: "all", or the parts by name.
    let shared: String?

    var finished: Bool {
        if let l = left { return l == 0 }
        if let t = total, let d = done { return d >= t }
        return false
    }

    /// Its counts in words, a noun at a time in its items' order ("398 of 398 z6 tiles", "18 of 18
    /// z3 tiles", "2 runs left"), as the build page and `scenic status` say them; an agent's before
    /// items: its own count.
    var counts: [String] {
        guard let items, !items.isEmpty else {
            if let t = total, let d = done, t > 0 { return ["\(grouped(Double(d))) of \(grouped(Double(t))) \(unit ?? "")"] }
            if let l = left, l > 0 { return ["\(l) left"] }
            return [finished ? "done" : "to come"]
        }
        var out: [String] = []
        var nouns: [String] = []
        for i in items {
            if nouns.contains(i.noun) { continue }
            nouns.append(i.noun)
            let of = items.filter { $0.noun == i.noun }
            if of.contains(where: { $0.total != nil }) {
                if of.allSatisfy({ $0.total != nil }) {
                    out.append("\(grouped(Double(of.reduce(0) { $0 + ($1.done ?? 0) }))) of \(grouped(Double(of.reduce(0) { $0 + ($1.total ?? 0) }))) \(i.noun)s")
                } else {
                    out.append("\(i.noun)s to come")
                }
            } else if of.allSatisfy({ $0.left != nil }) {
                let l = of.reduce(0) { $0 + ($1.left ?? 0) }
                if l > 0 { out.append("\(l) \(i.noun)\(l == 1 ? "" : "s") left") }
            } else {
                out.append("\(i.noun)s to come")
            }
        }
        return out.isEmpty ? [finished ? "done" : "to come"] : out
    }
}

/// A step's line of a stage (docs/pipeline.md §4): its targets done of total, or left.
struct Item: Decodable {
    let step: String
    /// Its step's title and the noun its targets are counted in, from the steps table.
    let title: String
    let noun: String
    let done: Int?
    let total: Int?
    let left: Int?

    var finished: Bool { left == 0 }
    var count: String {
        if let t = total, let d = done, t > 0 { return "\(grouped(Double(d))) of \(grouped(Double(t))) \(noun)s" }
        if let l = left { return l == 0 ? "done" : "\(l) \(noun)\(l == 1 ? "" : "s") left" }
        return "to come"
    }
}

struct JobProgress: Decodable {
    let done: Double
    let total: Double
    let unit: String
    let eta_s: Int?
    /// When it last moved on (seconds since the epoch; absent from older heartbeats).
    let moved_at: Int?
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

/// The helpers heard from lately (one not heard from for `outOfTouch` is left out).
func freshHelpers(_ r: Reply) -> [Helper] {
    (r.status?.helpers ?? []).filter { r.now - $0.beat <= outOfTouch }
}

/// The state, with a line saying it.
func classify(_ r: Reply?) -> (Kind, String) {
    guard let r = r else { return (.unknown, "The map's server on this Mac isn't answering") }
    guard let s = r.status else { return (.unknown, "No word from the lead (is the NAS reachable?)") }
    // A helper building counts as building, whatever the lead is doing.
    let helping = freshHelpers(r).filter { $0.firstJob != nil && $0.firstJob?.paused == nil }
    if r.now - s.beat > outOfTouch {
        if let h = helping.first { return (.building, clip("Building on \(h.host); lead out of touch since \(clock(s.beat))")) }
        return (.outOfTouch, "Lead out of touch since \(clock(s.beat))")
    }
    // Paused: stopping (a job finishing what it's on, here or on a helper), else paused.
    if let p = s.pause {
        let stopping = (s.firstJob.map { $0.paused == nil } ?? false) || (s.secondJob.map { $0.paused == nil } ?? false) || helping.contains { $0.firstJob?.pausing != nil }
        return (.paused, stopping ? "Pausing: finishing what it's on" : "Paused since \(clock(p.at))")
    }
    // (The lead's jobs: its second's beside its first.)
    if let j = s.firstJob {
        if j.paused != nil && (s.secondJob.map { $0.paused != nil } ?? true) {
            if let h = helping.first { return (.building, clip("Building on \(h.host); the lead's job paused")) }
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

/// The state with a line saying it, this Mac's agent's view of the pool considered: no lead for an
/// hour or more (its `alert`, fresh) is a problem, whatever the last status says, and the line says
/// why and what to do (docs/pool.md §10).
func state(_ r: Reply?, own: Own?) -> (Kind, String) {
    let now = r?.now ?? Int(Date().timeIntervalSince1970)
    // (This Mac paused alone, task #224: what this Mac does, whatever the pool's state; #628.)
    if r?.held_here != nil { return (.pausedHere, "This Mac is paused; the other Macs go on") }
    if let o = own, now - o.beat <= outOfTouch, let a = o.pool?.lead?.alert { return (.problem, a) }
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
        t += " · \(p.unit)"
    } else {
        // (Down, not to the nearest: 2.96 of 6 is 2.9, not 3.)
        let whole = p.done.rounded(.down) == p.done || p.total > 100
        let done = whole ? grouped(p.done.rounded(.down)) : String(format: "%.1f", (p.done * 10).rounded(.down) / 10)
        t += " · \(done) of \(grouped(p.total)) \(p.unit)"
    }
    if let e = p.eta_s, !paused { t += " · about \(duration(e)) left" }
    if let m = p.moved_at, !paused, now - m >= 15 * 60 { t += " · no further for \(duration(now - m))" }
    return t
}

/// The menu's items for this Mac paused alone (`scenic pause --here`, task #224): the ask while the
/// agent hasn't taken it up, then who paused it and Resume This Mac, else Pause This Mac. `held`:
/// what pressing it asks (pause: true, resume: false), nil for a line only.
func holdItems(_ r: Reply?) -> [(title: String, held: Bool?, tip: String)] {
    var out: [(title: String, held: Bool?, tip: String)] = []
    if r?.hold_asked == true { out.append(("Asked… (this Mac's agent takes it up within seconds)", nil, "")) }
    if let h = r?.held_here {
        out.append(("Paused since \(clock(h.at)) (from \(clip(h.by, 40)))", nil, ""))
        out.append(("Resume This Mac", false, "This Mac takes work again, as `scenic resume --here` does"))
    } else {
        out.append(("Pause This Mac", true, "Only this Mac: its jobs stop at their next safe point and are given back, it takes no new work and hands the lead away if it leads; the other Macs go on building"))
    }
    return out
}

/// The menu's lines for an answer, under the state's line (and this Mac's caches, from its agent's
/// own status).
func lines(_ r: Reply?, _ line: String, own: Own? = nil) -> [Line] {
    var out = [Line(text: line, style: .title)]
    if let o = r?.offline { out.append(Line(text: offlineText(o), style: .small)) }
    guard let r = r, let s = r.status else { return out }
    // When it'll be done and the map next gets new data (as the worker page and the map say it).
    if let f = s.forecast, r.now - s.beat <= outOfTouch {
        var t = f.done_at.map { "Done ≈ \(soon($0))" } ?? "No finish to forecast: \(f.why ?? "unknown")"
        if let rg = f.range, rg.count == 2, f.done_at != nil { t += " (\(soon(rg[0]))–\(soon(rg[1])))" }
        out.append(Line(text: t, style: .small))
        if let next = f.nextOnMap {
            let names = Dictionary((s.regions ?? []).map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
            let which = next.ids.prefix(3).map { names[$0] ?? $0 }.joined(separator: ", ") + (next.ids.count > 3 ? " and \(next.ids.count - 3) more" : "")
            out.append(Line(text: "Next on the map ≈ \(soon(next.at)): \(which)", style: .small))
        }
    }
    if let p = s.pause {
        let how = p.mode == "freeze" ? "every Mac's job frozen where it was" : "every Mac's job stops at its next safe point"
        out.append(Line(text: "From \(p.by), \(clock(p.at)): \(how); nothing new starts until it's resumed", style: .small))
    }
    if let j = s.firstJob {
        out.append(Line(text: j.what, style: .plain))
        // How far the job says it is, and the time it has left at its pace.
        var bar: Line? = nil
        if let p = j.progress {
            bar = Line(text: progressText(p, paused: j.paused != nil, now: r.now), style: .bar, fraction: p.total > 0 ? min(1, p.done / p.total) : 0)
        }
        // Its parts, done, under way and to come, the bar under the one under way (unless the bar
        // only counts the parts, which the list shows).
        if let parts = j.parts, !parts.isEmpty, let cur = j.part {
            let partsBar = j.progress.map { $0.unit.hasPrefix("parts") } ?? false
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
        // The log's last lines, without the terminal's colour codes.
        let plain = (j.tail ?? "").replacingOccurrences(of: "\u{1B}\\[[0-9;]*[A-Za-z]", with: "", options: .regularExpression)
        for l in plain.split(separator: "\n").suffix(3) where !l.trimmingCharacters(in: .whitespaces).isEmpty {
            out.append(Line(text: tildes(String(l).trimmingCharacters(in: .whitespaces)), style: .mono))
        }
    }
    // The lead's second slot's job, beside the first.
    if let j = s.secondJob {
        out.append(Line(text: "Beside it: \(j.what)", style: .plain))
        if let p = j.progress {
            out.append(Line(text: progressText(p, paused: j.paused != nil, now: r.now), style: .bar, fraction: p.total > 0 ? min(1, p.done / p.total) : 0))
        }
        if let p = j.paused { out.append(Line(text: p, style: .small)) } else if j.pausing != nil { out.append(Line(text: "Stopping at its next safe point (what it's on is kept)", style: .small)) }
        if let w = j.waiting { out.append(Line(text: w.prefix(1).uppercased() + w.dropFirst(), style: .small)) }
        out.append(Line(text: "Running \(duration(r.now - j.started)) (since \(clock(j.started)))", style: .small))
    }
    // The other Macs helping (z6 tile jobs only), each with its job.
    for h in freshHelpers(r) {
        guard let j = h.firstJob else {
            out.append(Line(text: "\(h.host) is helping; nothing for it to build now", style: .small))
            continue
        }
        out.append(Line(text: "\(h.host): \(j.what)", style: .plain))
        if let p = j.progress {
            out.append(Line(text: progressText(p, paused: j.paused != nil, now: r.now), style: .bar, fraction: p.total > 0 ? min(1, p.done / p.total) : 0))
        }
        if let p = j.paused { out.append(Line(text: "\(h.host): \(p)", style: .small)) } else if j.pausing != nil { out.append(Line(text: "\(h.host): stopping at its next safe point", style: .small)) }
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
    // This Mac's build caches: what a clear would free, and the last clear.
    if let c = own?.caches { out.append(Line(text: cachesLine(c), style: .small)) }
    // The pool: who leads, each member (this Mac's agent's view).
    if let v = own?.pool?.lead { out += poolLines(v, now: r.now) }
    // The build to the end: each step done, under way, or to come.
    if let steps = s.checklist, !steps.isEmpty {
        let now = [s.firstJob, s.secondJob].compactMap { $0.map { String($0.id.split(separator: " ").first ?? "") } }
        let finished = steps.filter(\.finished).count
        out.append(Line(text: "", style: .separator))
        let legend = steps.contains { !$0.finished && $0.shared != nil } ? "  ·  ⇄ any Mac can take part" : ""
        out.append(Line(text: "To the end: \(finished) of \(steps.count) stages done\(legend)", style: .header))
        for st in steps {
            if st.finished {
                out.append(Line(text: "✓ \(st.what)", style: .stepDone))
                continue
            }
            let count = ": " + st.counts.joined(separator: " · ")
            let running = now.contains { st.steps.contains($0) }
            let mark = st.shared != nil ? "  ⇄" : ""
            out.append(Line(text: "\(running ? "▸" : "○") \(st.what)\(count)\(mark)", style: running ? .stepNow : .stepToDo))
            // Its steps, each the same: done, under way or to come, its count in its noun.
            for it in st.items ?? [] {
                let here = now.contains(it.step)
                out.append(Line(text: "      \(it.finished ? "✓" : here ? "▸" : "○") \(it.title): \(it.count)", style: .small))
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
        // Those still building, as the Regions panel says them: on the map as their tiles come, and
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
    if let f = c.cleared, f.why_not == nil { t += " · cleared \(clock(f.at)), \(gb(f.bytes)) freed" }
    return t
}

/// A repeating timer on the main run loop in its common modes: it fires while a menu is open or a
/// popover tracks the mouse too (the default mode alone is suspended then, and what it held up all
/// came at once when the menu closed).
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

/// Where a menu opens under a view: its bottom left corner, a few points down.
func under(_ v: NSView) -> NSPoint {
    NSPoint(x: 0, y: v.isFlipped ? v.bounds.height + 4 : -4)
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

/// A lead item's action, carried by its menu item.
final class LeadBox: NSObject {
    let action: LeadAction
    init(_ a: LeadAction) { action = a }
}

/// A view whose subviews lay out from the top.
final class Flipped: NSView {
    override var isFlipped: Bool { true }
}

/// The panel: the build page in a popover under the icon (docs/building.md §2, Status). This Mac's own
/// agent's page (`Panel.page`, its port 8090: every Mac's agent serves it, from its own read of the
/// build's state, docs/pool.md §10) with `?view`, so it only ever watches (web/work/index.html), in
/// a web view of its own that keeps nothing (no stored data: never a page that helps); made as the
/// popover first opens and kept for the next opening (#408), its polling stopped while closed. Its
/// height is the page's, within 900 pt and the screen; its links out of the page open in the
/// browser. While this Mac's agent doesn't answer, one plain line says so. A footer: the state in a
/// line (and a notice when the agent runs another app than this one), Reload, and "⋯", the menu.
final class Panel: NSObject, NSPopoverDelegate, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
    static let pageWidth: CGFloat = 780
    static let footerHeight: CGFloat = 30
    /// This Mac's agent's build page (a sandbox's own: Info.plist's ScenicPage).
    static let page = info["ScenicPage"] as? String ?? "http://127.0.0.1:8090/work/"
    static var viewURL: URL { URL(string: page + "?view")! }
    let popover = NSPopover()
    let menu: () -> NSMenu
    var web: WKWebView?
    weak var anchor: NSView?
    /// When it last closed: the click on the icon that closed it doesn't open it again.
    var closedAt = Date.distantPast
    /// While it's open: the monitor of clicks in other apps, which close it.
    var outside: Any?
    /// The status as last polled: the footer's line.
    var line = ""
    /// The agent's app, when it isn't this one's (the swarm answer's `app`): said in the footer.
    var otherApp: String?
    /// The content: clear, so the popover's own translucent material shows through, the same
    /// under the body and the arrow.
    let root = NSView(frame: NSRect(x: 0, y: 0, width: Panel.pageWidth, height: 400))
    let state = NSTextField(labelWithString: "")
    let reload = NSButton(title: "Reload", target: nil, action: nil)
    let more = NSButton(title: "⋯", target: nil, action: nil)
    /// The one line in the page's place while this Mac's agent doesn't answer.
    var down: NSTextField?

    init(menu: @escaping () -> NSMenu) {
        self.menu = menu
        super.init()
        // The system's light or dark switched (open or not): the popover, its glass and the page
        // follow at once.
        appearanceWatch = NSApp.observe(\.effectiveAppearance, options: [.new]) { [weak self] _, _ in
            DispatchQueue.main.async { self?.applyAppearance() }
        }
        popover.behavior = .transient
        popover.animates = false
        popover.delegate = self
        let footer = NSView(frame: NSRect(x: 0, y: 0, width: root.frame.width, height: Panel.footerHeight))
        footer.autoresizingMask = [.width]
        state.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        state.textColor = .secondaryLabelColor
        state.lineBreakMode = .byTruncatingTail
        state.frame = NSRect(x: 12, y: 7, width: footer.frame.width - 130, height: 16)
        state.autoresizingMask = [.width]
        for (b, x, w) in [(reload, 112, 64), (more, 44, 32)] as [(NSButton, CGFloat, CGFloat)] {
            b.bezelStyle = .accessoryBarAction
            b.controlSize = .small
            b.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            b.target = self
            b.frame = NSRect(x: footer.frame.width - x, y: 4, width: w, height: 22)
            b.autoresizingMask = [.minXMargin]
            footer.addSubview(b)
        }
        reload.action = #selector(reloadPage)
        reload.toolTip = "Load the build page again"
        more.action = #selector(showMenu)
        more.toolTip = "Pause, the lead, disk room, the map…"
        footer.addSubview(state)
        root.addSubview(footer)
        let vc = NSViewController()
        vc.view = root
        popover.contentViewController = vc
    }

    func toggle(from button: NSView) {
        if popover.isShown {
            close()
            return
        }
        if Date().timeIntervalSince(closedAt) < 0.3 { return }
        anchor = button
        NSApp.activate()
        // (A click in another app closes it too: the cooperative `activate()` can be declined for
        // a menu bar app, and then the transient popover never sees that click.)
        outside = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]) { [weak self] _ in self?.close() }
        // (The page kept from the last opening keeps its size, within maxHeight; nothing yet, a
        // blank of 400.)
        if web == nil { popover.contentSize = NSSize(width: Panel.pageWidth, height: 400) }
        else if popover.contentSize.height > maxHeight { popover.contentSize.height = maxHeight }
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        applyAppearance()
        popover.contentViewController?.view.window?.makeKey()
        load()
        checkApp()
    }

    func close() {
        if popover.isShown { popover.performClose(nil) }
    }

    /// Watches the app's appearance (the system's, or SCENIC_STATUS_APPEARANCE's).
    var appearanceWatch: NSKeyValueObservation?

    /// The panel in the system's light or dark, not the menu bar's (which follows the wallpaper),
    /// set on each opening and again on each switch: the popover's and the content's appearance
    /// (the web view's prefers-color-scheme follows the content's, so the page re-styles itself
    /// live), and the popover's own glass (one NSGlassEffectView under the arrow and the body
    /// alike) in its regular style, tinted with the window's colour of that appearance, resolved
    /// now, so the dim text stays readable over a bright window behind. (Before, both were set at
    /// opening only: a switch while open, or the glass's tint kept from the other appearance, left
    /// a grey veil over a light page.)
    /// (Regular, not clear (#485): the popover puts its glass back to regular on each resize, so
    /// regular was the look nearly always; a clear set after the last resize (a light/dark switch
    /// while open, a kept page reopened at its height) stayed, and showed the backdrop unblurred.)
    func applyAppearance() {
        let a = NSApp.effectiveAppearance
        popover.appearance = a
        root.appearance = a
        web?.appearance = a
        var tint = NSColor.windowBackgroundColor
        a.performAsCurrentDrawingAppearance { tint = NSColor(cgColor: NSColor.windowBackgroundColor.cgColor) ?? tint }
        for case let g as NSGlassEffectView in popover.contentViewController?.view.window?.contentView?.superview?.subviews ?? [] {
            g.appearance = a
            g.style = .regular
            g.tintColor = tint.withAlphaComponent(0.6)
        }
        root.needsDisplay = true
    }

    /// The tallest the panel's content may be: 900 pt, or the screen's room under the menu bar if
    /// less; a taller page scrolls inside it.
    var maxHeight: CGFloat {
        min(900, ((anchor?.window?.screen ?? NSScreen.main)?.visibleFrame.height ?? 800) - 40)
    }

    /// The page: the one kept from the last opening loaded again in place (still shown meanwhile),
    /// else a web view made for it.
    func load() {
        setDown(nil)
        let url = Panel.viewURL
        if let w = web {
            w.load(URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData))
            return
        }
        let conf = WKWebViewConfiguration()
        // Not shared with Safari, nor kept on disk: no "helping" kept beyond the app's run.
        conf.websiteDataStore = .nonPersistent()
        // The page's height, as it changes, for the panel's.
        let measure = "new ResizeObserver(() => webkit.messageHandlers.size.postMessage(Math.ceil(document.body.getBoundingClientRect().height))).observe(document.body);"
        conf.userContentController.addUserScript(WKUserScript(source: measure, injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        conf.userContentController.add(self, name: "size")
        // The page's own background let go, so the popover's material shows through (its cards
        // keep theirs, so their text stays as legible as in a browser).
        // (At the start, so no opaque flash on a load or Reload; `!important`, so no later rule of
        // the page's undoes it.)
        // Its cards, boxes and bars (index.html's --panel, --bg) partly see-through too, so what's
        // behind the popover shows, the text on them still clear.
        let css = "html,body{background:transparent!important}"
            + ":root{--bg:rgba(14,16,19,.4)!important;--panel:rgba(22,26,31,.55)!important}"
            + "@media (prefers-color-scheme: light){:root{--bg:rgba(244,246,248,.4)!important;--panel:rgba(255,255,255,.55)!important}}"
        let clear = "const s = document.createElement('style'); s.textContent = '\(css)'; document.documentElement.appendChild(s);"
        conf.userContentController.addUserScript(WKUserScript(source: clear, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let w = WKWebView(frame: NSRect(x: 0, y: Panel.footerHeight, width: root.frame.width, height: root.frame.height - Panel.footerHeight), configuration: conf)
        w.autoresizingMask = [.width, .height]
        // (Without this the web view paints its own opaque backdrop under the page, whatever the
        // page's and the under-page colours: no public switch for it on macOS.)
        w.setValue(false, forKey: "drawsBackground")
        w.underPageBackgroundColor = .clear
        w.navigationDelegate = self
        w.uiDelegate = self
        w.appearance = NSApp.effectiveAppearance
        root.addSubview(w)
        web = w
        w.load(URLRequest(url: url))
    }

    /// How often the app asks /work/swarm in the background: under the agent's 60 s ASKED_FOR_S.
    static let keepWarmS: TimeInterval = 50

    /// The agent's app against this one's: the swarm answer's `app` (an agent from #409 on), a
    /// notice in the footer when they differ. Also the background ask that keeps this Mac's page
    /// view reading (`keepWarmS`); a 503 (not read yet) or no answer changes nothing here.
    func checkApp() {
        guard let u = URL(string: Panel.page + "swarm") else { return }
        var req = URLRequest(url: u)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = Data("{}".utf8)
        req.timeoutInterval = 5
        URLSession.shared.dataTask(with: req) { data, resp, _ in
            guard (resp as? HTTPURLResponse)?.statusCode == 200, let d = data,
                  let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any], let app = o["app"] as? String else { return }
            let mine = bundleVersion(Bundle.main.bundleURL)
            DispatchQueue.main.async {
                self.otherApp = (mine != nil && app != mine) ? app : nil
                self.showState()
            }
        }.resume()
    }

    /// `--show panel`: once the page has loaded, whether it scrolls sideways at the panel's width.
    var checkWidth = false

    func webView(_ w: WKWebView, didFinish n: WKNavigation!) {
        guard checkWidth else { return }
        after(3) {
            w.evaluateJavaScript("[document.documentElement.scrollWidth, document.documentElement.clientWidth, document.getElementById('device')?.hidden, !!document.getElementById('help')]") { r, _ in
                print("page width (scroll, client), This device hidden, help button: \(r ?? "?")")
                fflush(stdout)
            }
        }
    }

    /// The web view gone, and its page with it.
    func dropWeb() {
        guard let w = web else { return }
        w.stopLoading()
        w.navigationDelegate = nil
        w.uiDelegate = nil
        w.configuration.userContentController.removeScriptMessageHandler(forName: "size")
        w.removeFromSuperview()
        web = nil
    }

    /// This Mac's agent doesn't answer (`why`: what the load said, for the log): the page dropped,
    /// one plain line in its place; nil, the line gone.
    func setDown(_ why: String?) {
        down?.removeFromSuperview()
        down = nil
        guard let why else { return }
        NSLog("scenic-status: the build page (%@): %@", Panel.page, why)
        dropWeb()
        let l = NSTextField(labelWithString: "The build's agent isn't running on this Mac")
        l.textColor = .secondaryLabelColor
        l.frame = NSRect(x: 16, y: Panel.footerHeight + 16, width: root.frame.width - 32, height: 20)
        l.autoresizingMask = [.width]
        root.addSubview(l)
        down = l
        popover.contentSize = NSSize(width: Panel.pageWidth, height: 52 + Panel.footerHeight)
        reload.title = "Retry"
    }

    /// The status polled again: the footer's line.
    func update(line: String) {
        self.line = line
        showState()
    }

    func showState() {
        state.stringValue = otherApp.map { "This Mac's agent runs app \($0), not this app's: quit and open Scenic again · " + line } ?? line
    }

    @objc func reloadPage() {
        reload.title = "Reload"
        if let w = web, down == nil { w.reload() } else { load() }
        checkApp()
    }

    @objc func showMenu(_ sender: NSButton) {
        menu().popUp(positioning: nil, at: under(sender), in: sender)
    }

    // MARK: the popover

    func popoverDidClose(_ n: Notification) {
        if let m = outside { NSEvent.removeMonitor(m) }
        outside = nil
        // The kept page polls no more while closed: its loading stopped and its timers cleared
        // (each opening loads it again, so nothing is lost; it stays drawn meanwhile).
        if let w = web {
            w.stopLoading()
            w.evaluateJavaScript("for (let i = setTimeout(() => {}, 0); i > 0; i--) { clearTimeout(i); clearInterval(i); }")
        }
        closedAt = Date()
    }

    // MARK: the page

    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        guard m.name == "size", let h = (m.body as? NSNumber).map({ CGFloat($0.doubleValue) }), web != nil, down == nil else { return }
        fit(h)
    }

    /// The popover sized to the page's height, within maxHeight.
    func fit(_ h: CGFloat) {
        let height = min(max(h, 160), maxHeight - Panel.footerHeight) + Panel.footerHeight
        if abs(popover.contentSize.height - height) >= 1 || popover.contentSize.width != Panel.pageWidth {
            popover.contentSize = NSSize(width: Panel.pageWidth, height: height)
        }
    }

    /// The page's own addresses stay in it; any other (the map, another site) opens in the browser.
    func webView(_ w: WKWebView, decidePolicyFor a: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let u = a.request.url else { return decisionHandler(.allow) }
        let base = Panel.viewURL
        let inPage = u.scheme == base.scheme && u.host?.lowercased() == base.host?.lowercased() && u.port == base.port && u.path.hasPrefix("/work")
        if inPage || ["about", "blob", "data"].contains(u.scheme ?? "") {
            decisionHandler(.allow)
            return
        }
        decisionHandler(.cancel)
        NSLog("scenic-status: %@ isn't the build page's (%@): in the browser", u.absoluteString, base.absoluteString)
        if a.targetFrame?.isMainFrame ?? true { NSWorkspace.shared.open(u) }
    }

    /// The page's confirm() (a cancel, freezing the build): asked natively, the panel's own. The
    /// popover kept open meanwhile (a transient one closes as the alert takes the keyboard, and
    /// closing stops the page's timers), then made the key window again, the page going on.
    func webView(_ w: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void) {
        let a = NSAlert()
        a.messageText = message
        a.addButton(withTitle: "OK")
        a.addButton(withTitle: "Cancel")
        popover.behavior = .applicationDefined
        let ok = a.runModal() == .alertFirstButtonReturn
        popover.behavior = .transient
        if popover.isShown { popover.contentViewController?.view.window?.makeKey() }
        completionHandler(ok)
    }

    /// A link to a new window: in the browser.
    func webView(_ w: WKWebView, createWebViewWith c: WKWebViewConfiguration, for a: WKNavigationAction, windowFeatures f: WKWindowFeatures) -> WKWebView? {
        if let u = a.request.url, ["http", "https", "mailto"].contains(u.scheme ?? "") { NSWorkspace.shared.open(u) }
        return nil
    }

    func webView(_ w: WKWebView, didFailProvisionalNavigation n: WKNavigation!, withError e: Error) {
        // (A load cancelled by the next one, the reopening's or Reload's, isn't a failure.)
        if w === web, (e as NSError).code != NSURLErrorCancelled { setDown(e.localizedDescription) }
    }

    func webView(_ w: WKWebView, didFail n: WKNavigation!, withError e: Error) {
        if w === web, (e as NSError).code != NSURLErrorCancelled { setDown(e.localizedDescription) }
    }

    func webViewWebContentProcessDidTerminate(_ w: WKWebView) {
        // (Its process ended: made again, as the next opening would.)
        if w === web {
            dropWeb()
            if popover.isShown { load() }
        }
    }
}

/// What notifications compare: the job, whether it's paused, the last finished job, out of touch.
struct Seen {
    var job: String?
    var paused: Bool
    var lastEnded: Int
    var outOfTouch: Bool
    var buildPaused: Bool
    /// The gate's units held.
    var held: Set<String> = []
}

/// This Mac's ask to its agent (crates/pipeline/src/control.rs), while it waits to be taken up: to
/// pause (true) or go on (false).
let askFile = "pause-request.json"
func pendingAsk() -> Bool? {
    guard let d = try? Data(contentsOf: home.appendingPathComponent("agent").appendingPathComponent(askFile)),
          let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return nil }
    return !(o["pause"] is NSNull || o["pause"] == nil)
}

/// The pool as this Mac's agent's pool last stepped it (`agent/pool/view.json`: crates/pipeline/
/// src/agent/pool.rs PoolFile), written each of its steps, every 20 s or so whatever the agent's
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
        return Own(host: own.host, beat: own.beat, caches: own.caches, pool: v.pool)
    }
    return own
}

/// This Mac's ask to its agent to clear its build caches (room.rs ClearRequest), while it waits or
/// is under way (taken up: renamed aside until it's answered).
let clearFile = "clear-request.json"
func clearAsked() -> Bool {
    [clearFile, "\(clearFile).taken"].contains { FileManager.default.fileExists(atPath: home.appendingPathComponent("agent").appendingPathComponent($0).path) }
}

/// The owner's disk room target on this Mac (room.rs Target, `scenic room`), in its agent's folder:
/// its bytes, as set now (nil: off).
let targetFile = "room-target.json"
func roomTarget() -> Int? {
    guard let d = try? Data(contentsOf: home.appendingPathComponent("agent").appendingPathComponent(targetFile)),
          let t = try? JSONDecoder().decode(RoomTarget.self, from: d), t.size > 0 else { return nil }
    return t.size
}

/// The free space on the disk of the agent's folder, now (as its agent measures it: statfs's).
func diskFree() -> Int? {
    (try? FileManager.default.attributesOfFileSystem(forPath: home.path))?[.systemFreeSize] as? Int
}

/// The size of the disk of the agent's folder.
func diskSize() -> Int? {
    (try? FileManager.default.attributesOfFileSystem(forPath: home.path))?[.systemSize] as? Int
}

/// The room target's presets (GB).
let roomPresets = [10, 20, 50, 100, 150, 200, 300]

/// The menu's Disk Room item (none without an agent here): its title, with the free space and the
/// target; its tooltip (why the disk is short of it, when it stays so); and its submenu's choices
/// (a preset's GB, 0 for Off), with the one set checked: none the disk (`disk`, its size) can't hold,
/// as `scenic room` refuses them.
func roomItem(_ own: Own?, target: Int?, free: Int?, disk: Int?) -> (title: String, tip: String, choices: [(title: String, gb: Int, on: Bool)])? {
    guard own?.caches != nil else { return nil }
    let title = "Disk Room: " + diskWords(free: free, target: target, reserved: (own?.caches?.room?.reserved_mb ?? 0) << 20)
    let r = own?.caches?.room
    var tip = "The free space this Mac's agent keeps: it frees its build caches to it, as far as needed, jobs running or not (what they use stays), and starts no job that would cross it, until it's lowered or off."
    if let t = target, r?.target?.size == t, let why = r?.short { tip = "Short of it: \(why)" }
    if let t = target, r?.target?.size == t, let f = r?.toward, f.bytes > 0 { tip += "\nFreed toward it \(clock(f.at)): \(freedText(f))" }
    var choices = roomPresets.filter { p in disk.map { p << 30 < $0 } ?? true }.map { (title: "Keep \($0) GB Free", gb: $0, on: target == $0 << 30) }
    if let t = target, !choices.contains(where: \.on) { choices.append((title: "Keep \(gb(t)) Free", gb: -1, on: true)) }
    choices.append((title: "Off", gb: 0, on: target == nil))
    return (title, tip, choices)
}

/// The disk in words (task #391, as coord::disk_words): "38 GB free · 18 GB for jobs (20 GB room
/// target)": what's left for jobs, the free space less the owner's room target and its jobs'
/// reservations (no fixed reserve: task #421); nothing kept, no parentheses.
func diskWords(free: Int?, target: Int?, reserved: Int = 0) -> String {
    let g = { (b: Int) in "\(Int((Double(b) / Double(1 << 30)).rounded()))\u{00A0}GB" }
    guard let f = free else { return "? free" }
    var kept: [String] = []
    if let t = target { kept.append("\(g(t)) room target") }
    if reserved > 0 { kept.append("\(g(reserved)) reserved by its jobs") }
    let why = kept.isEmpty ? "" : " (\(kept.joined(separator: ", ")))"
    return "\(g(f)) free · \(g(max(0, f - (target ?? 0) - reserved))) for jobs\(why)"
}

/// The memory this Mac gives the build's jobs together (crates/pipeline/src/agent/buildmem.rs,
/// `scenic memory`), in its agent's folder: its MB, as set now (nil: the memory guard's limit).
let memoryFile = "build-memory.json"
struct BuildMemory: Decodable {
    let mb: Double
    let by: String
    let at: Int
}
func buildMemory() -> Int? {
    guard let d = try? Data(contentsOf: home.appendingPathComponent("agent").appendingPathComponent(memoryFile)),
          let m = try? JSONDecoder().decode(BuildMemory.self, from: d), m.mb >= 1 else { return nil }
    return Int(min(m.mb, Double(1 << 40)))
}

/// The memory guard's limit on a Mac of `total` MB (memguard.rs limit_mb): an eighth kept, 4 GB at least.
func guardLimitMB(_ total: Int) -> Int { max(0, total - max(total / 8, 4096)) }

/// The build memory's presets (GB).
let memoryPresets = [4, 6, 8, 10, 12, 16, 24, 32, 48, 64]

/// The menu's Memory item (none without an agent here): its title, the memory the build's jobs get
/// together (the amount set, within the guard's limit; none set, the limit); its tooltip; its
/// submenu's choices (a preset's GB under the limit, 0 for the limit alone), the one set checked.
func memoryItem(_ own: Own?, set: Int?, totalMB: Int) -> (title: String, tip: String, choices: [(title: String, gb: Int, on: Bool)])? {
    guard own?.caches != nil else { return nil }
    let limit = guardLimitMB(totalMB)
    let g = { (mb: Int) in String(format: "%.0f\u{00A0}GB", Double(mb) / 1024) }
    let title: String
    // (As the page says it: the limit named only where it holds the build's memory under it.)
    if let m = set, limit <= 0 { title = "Memory: \(g(m)) for the build" }
    else if let m = set, m < limit { title = "Memory: \(g(m)) for the build (limit \(g(limit)))" }
    else { title = "Memory: \(g(limit)) for the build (the memory guard's limit)" }
    let tip = "The memory this Mac's jobs get together, whether you're using the Mac or not; the memory guard's limit (\(g(limit))) stays the ceiling."
    var choices = memoryPresets.filter { limit == 0 || $0 << 10 <= limit }.map { (title: "Give the Build \($0) GB", gb: $0, on: set == $0 << 10) }
    if let m = set, !choices.contains(where: \.on) { choices.append((title: "Give the Build \(g(m))", gb: -1, on: true)) }
    choices.append((title: "The Memory Guard's Limit (\(g(limit)))", gb: 0, on: set == nil))
    return (title, tip, choices)
}

/// This Mac's memory (MB).
func macMemoryMB() -> Int { Int(ProcessInfo.processInfo.physicalMemory >> 20) }

/// "51.2 GB", or under a GB, "350 MB" (number and unit never split across lines).
func gb(_ b: Int) -> String {
    b >= 1 << 30 ? String(format: "%.1f\u{00A0}GB", Double(b) / Double(1 << 30)) : "\(b >> 20)\u{00A0}MB"
}

/// What a clear freed, by cache, the most first: "canopy squares 45.0 GB, raw terrain
/// tiles 24.1 GB", and what stays.
func freedText(_ f: Freed) -> String {
    let words = ["canopy": "canopy squares", "terrain": "raw terrain tiles", "blobs": "copies of the records' files", "base": "base packs", "dem": "the DEM seed", "copies": "copies of the NAS's files", "heritage": "the heritage jobs' planet clip"]
    var t = (f.freed ?? [:]).filter { $0.value > 0 }.sorted { $0.value > $1.value }.map { "\(words[$0.key] ?? $0.key) \(gb($0.value))" }.joined(separator: ", ")
    if t.isEmpty { t = "nothing" }
    if let l = f.left, l > 0 { t += "; \(gb(l)) kept (the NAS hasn't it yet)" }
    return t
}

/// The menu's item for this Mac's build caches (none without an agent here): Clear the Build's
/// Caches with what it would free, enabled once the build is done (why not, in
/// its tooltip); while an ask waits, that it's clearing.
func cachesItem(_ own: Own?, now: Int, asked: Bool) -> (title: String, enabled: Bool, tip: String)? {
    guard let own = own, let c = own.caches else { return nil }
    if asked { return ("Clearing the Build's Caches… (asked; this Mac's agent does it now, jobs running keeping what they use)", false, "") }
    let title = "Clear the Build's Caches" + (c.clearable.map { " (\(gb($0)))" } ?? "")
    if now - own.beat > outOfTouch { return (title, false, "This Mac's agent hasn't written its status since \(clock(own.beat)): is it running?") }
    if let why = c.why_not { return (title, false, "Not now: \(why)") }
    if let n = c.clearable, n < 50 << 20 { return (title, false, "They hold nothing now") }
    return (title, true, "Deletes this Mac's copies of what the NAS keeps (canopy squares, raw terrain tiles, base packs, the DEM seed …): later jobs copy back from the NAS, or make again, what they need. The map's offline copy stays.")
}

final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    lazy var item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    var reply: Reply?
    var seen: Seen?
    /// This Mac's agent's own status (its caches), read with each answer, and its last clear's
    /// and declined ask's times as last told.
    var own: Own?
    var seenCaches: (cleared: Int, declined: Int)?
    var polling = false
    /// The app's build service, the map server, and the window (service.swift).
    let build = BuildService()
    let mapServer = MapServer()
    lazy var window: StatusWindow = {
        let w = StatusWindow()
        w.onStart = { [weak self] in self?.startServer() }
        w.onStop = { [weak self] in self?.stopServer() }
        return w
    }()
    /// Quitting goes ahead without asking: the build service stopped as asked, or the app replaced
    /// by an update (the build service left running).
    var mayQuit = false
    /// Whether this run manages the services (the app opened, not a --show or --menu-proof run).
    var managing = false
    /// `--menu-proof`: its polls and notifications printed with the time, a menu held open between.
    var proof = false
    /// `--show panel|menu`: opened once the first answer is in, the panel closed 20 s later (for
    /// screenshots, and what the panel costs open and closed).
    var showAtStart: String?
    /// Whether notifications are posted (not printed, as --replay's are).
    var sinkIsCenter = true
    /// Where notifications go: posted, or printed (--replay).
    var sink: (String, String) -> Void = { title, body in
        let c = UNMutableNotificationContent()
        c.title = title
        c.body = body
        c.threadIdentifier = "build"
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
    }

    func applicationDidFinishLaunching(_ n: Notification) {
        // Known to Launch Services by its bundle (notifications need it), also when its executable
        // is started directly (--show, --menu-proof).
        LSRegisterURL(Bundle.main.bundleURL as CFURL, true)
        if !proof && showAtStart == nil { openApp() }
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        // (A device's asks to help, from before pages helped with no key: taken away.)
        center.getDeliveredNotifications { ns in
            let asks = ns.map(\.request.identifier).filter { $0.hasPrefix("ask-") }
            if !asks.isEmpty { center.removeDeliveredNotifications(withIdentifiers: asks) }
        }
        center.requestAuthorization(options: [.alert, .sound]) { _, _ in }
        // (SCENIC_STATUS_APPEARANCE: light or dark whatever the system's, for screenshots.)
        switch ProcessInfo.processInfo.environment["SCENIC_STATUS_APPEARANCE"] {
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        default: break
        }
        if let b = item.button {
            // A click opens the panel (the build page); a right-click, or Control-click, the menu.
            b.target = self
            b.action = #selector(clicked)
            b.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        show()
        poll()
        every(5) { [weak self] in self?.poll() }
        every(60) { [weak self] in self?.updateIfReplaced() }
        // (This Mac's page view reads the build's state only while asked within the last minute,
        // pageview.rs ASKED_FOR_S: asked every `keepWarmS`, its snapshot stays fresh and the panel
        // has the build the moment it opens. The NAS cost is the agent's own 12 s read loop, kept
        // going while this app runs.)
        panel.checkApp()
        every(Panel.keepWarmS) { [weak self] in self?.panel.checkApp() }
        if proof { menuProof() }
        if let what = showAtStart {
            after(2) {
                self.panel.checkWidth = true
                if what == "menu" { self.popMenu() } else if let b = self.item.button { self.panel.toggle(from: b) }
                // (The panel closed again after 20 s: what it leaves behind measured.)
                after(20) {
                    self.panel.close()
                    print("\(stamp()) panel closed")
                    fflush(stdout)
                }
            }
        }
    }

    /// `--menu-proof`: after a few seconds the menu opens and stays open 25 s (closed by a timer of
    /// its own), then it quits 12 s later; each poll and notification printed with the time. With
    /// the timers in the common modes the polls go on every 5 s while the menu is open, and each
    /// notification comes as its answer does, not all at once as the menu closes.
    func menuProof() {
        sink = { title, body in print("\(stamp()) notify: \(title) — \(body)") }
        sinkIsCenter = false
        after(6) {
            print("\(stamp()) menu opens")
            let menu = self.currentMenu()
            after(25) { menu.cancelTracking() }
            self.popMenu(menu)
            print("\(stamp()) menu closed")
            after(12) { exit(0) }
        }
    }

    // MARK: the app: its build service, the map server, quitting, updates (service.swift)

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
        appMenu.addItem(withTitle: "Show the \(name) Window", action: #selector(showWindow), keyEquivalent: "0").target = self
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
        mapServer.refresh()
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
    /// it has stopped), the map server's state, the window.
    func tendServices() {
        // (Not in --show and --menu-proof's runs: the services left alone.)
        guard managing else { return }
        recheckSwitchOver()
        build.tend()
        mapServer.refresh()
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
        window.update(build: build, map: mapServer, job: line)
    }

    @objc func showWindow() {
        updateWindow()
        window.show()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { showWindow() }
        return true
    }

    @objc func startServer() {
        mapServer.start()
        updateWindow()
        poll()
    }

    @objc func stopServer() {
        let a = NSAlert()
        a.messageText = "Stop the map server?"
        a.informativeText = "The map stops loading on this Mac and on devices that use it (an iPad) until it's started again here, or at the next login. The build service goes on."
        a.addButton(withTitle: "Stop the Map Server")
        a.addButton(withTitle: "Cancel")
        NSApp.activate()
        guard a.runModal() == .alertFirstButtonReturn else { return }
        mapServer.stop()
        updateWindow()
    }

    /// A newer version installed (the server put it under app/): this app replaced by it and opened
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
    /// service stopped. No job running: at once. A job running: asked, "Finish This Job First" (the
    /// app quits once the service has stopped) or "Stop Now". A logout, restart or shutdown: "Stop
    /// Now" without asking. The agent hands the lead away itself as it quits (control.rs Quit).
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
        a.addButton(withTitle: "Finish This Job First")
        a.addButton(withTitle: "Stop Now")
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

    @objc func startServerItem(_ sender: NSMenuItem) { startServer() }
    @objc func stopServerItem(_ sender: NSMenuItem) { stopServer() }

    func poll() {
        tendServices()
        if polling { return }
        polling = true
        var req = URLRequest(url: server.appendingPathComponent("api/build"))
        req.timeoutInterval = 4
        URLSession.shared.dataTask(with: req) { data, resp, _ in
            let ok = (resp as? HTTPURLResponse)?.statusCode == 200
            let r = ok ? data.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) } : nil
            DispatchQueue.main.async {
                if self.proof { print("\(stamp()) poll: \(r.map { classify($0).1 } ?? "no answer")") }
                self.polling = false
                self.reply = r
                self.own = ownStatus()
                self.notifyChanges()
                self.show()
            }
        }.resume()
    }

    // MARK: the icon and the menu

    func show() {
        let (kind, line) = state(reply, own: own)
        if let b = item.button {
            let held = heldInputs(reply)
            b.image = iconImage(kind, held: !held.isEmpty, line: line)
            b.toolTip = held.isEmpty ? line : "\(line)\n" + held.map { "Held at the gate: \(inputLine($0))" }.joined(separator: "\n")
        }
        panel.update(line: line)
    }

    /// The panel: the build page in a popover, while it's open.
    lazy var panel = Panel(menu: { [weak self] in self?.currentMenu() ?? NSMenu() })

    @objc func clicked(_ sender: NSStatusBarButton) {
        let e = NSApp.currentEvent
        if e?.type == .rightMouseUp || e?.modifierFlags.contains(.control) == true {
            panel.close()
            popMenu()
        } else {
            panel.toggle(from: sender)
        }
    }

    /// The menu, under the icon, where the system puts a status item's menu (given to the item for
    /// this click alone: a click opens the panel).
    func popMenu(_ m: NSMenu? = nil) {
        item.menu = m ?? currentMenu()
        item.button?.performClick(nil)
        item.menu = nil
    }

    func currentMenu() -> NSMenu {
        recheckSwitchOver()
        let (kind, line) = state(reply, own: own)
        return menu(kind, line)
    }

    /// The build page's address for another device: the one this Mac's coordinator writes while
    /// it leads or brokers (HTTPS through Tailscale when it's served so); none otherwise.
    func pageToShare() -> String? {
        if let page = try? String(contentsOf: home.appendingPathComponent("agent/coord/page"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), !page.isEmpty { return page }
        return nil
    }

    /// The menu (a right-click on the icon, or the panel's "⋯"): what the page can't say (the state
    /// in a line, this Mac's downloads and its build caches), then the controls.
    func menu(_ kind: Kind, _ line: String) -> NSMenu {
        let m = NSMenu()
        m.autoenablesItems = false
        var info = [Line(text: line, style: .title)]
        if let o = reply?.offline { info.append(Line(text: offlineText(o), style: .small)) }
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
            let acc = NSMenuItem(title: warns.isEmpty ? "Errors: fix or remove the files" : "Accept \(warns.count == 1 ? "the Warning" : "\(warns.count) Warnings")…", action: warns.isEmpty ? nil : #selector(acceptInputs), keyEquivalent: "")
            acc.target = self
            acc.isEnabled = !warns.isEmpty
            acc.representedObject = v.unit
            sub.addItem(acc)
            do { let page = Panel.page
                let show = NSMenuItem(title: "Show on the Build Page", action: #selector(openPage), keyEquivalent: "")
                show.target = self
                show.representedObject = page.hasSuffix("#overview") ? page : page + "#inputs"
                sub.addItem(show)
            }
            it.submenu = sub
            m.addItem(it)
        }
        if !held.isEmpty { m.addItem(.separator()) }
        // The whole build paused (every Mac), or going on: an ask to this Mac's agent, which passes it
        // on to the lead (crates/pipeline/src/control.rs). Option: at once, frozen where it is.
        // (Paused as this Mac knows it: the lead's, or, a helper's own, its own.)
        let ownHelper = reply?.status?.helpers?.first { $0.here == true }
        let pausedHere = reply?.status?.pause != nil || ownHelper?.pause != nil
        let asked = pendingAsk()
        if let a = asked {
            let it = NSMenuItem(title: a ? "Pausing… (asked; the lead takes it up within seconds)" : "Resuming… (asked)", action: nil, keyEquivalent: "")
            it.isEnabled = false
            m.addItem(it)
        }
        // (The opposite of what was asked, or of the state, is always there: an ask is never stuck.)
        if asked == true || (asked == nil && pausedHere) {
            let it = NSMenuItem(title: "Resume Building", action: #selector(resumeBuild), keyEquivalent: "")
            it.target = self
            m.addItem(it)
        } else {
            let it = NSMenuItem(title: "Pause Building", action: #selector(pauseBuild), keyEquivalent: "")
            it.target = self
            it.toolTip = "Every Mac's running job stops at its next safe point (a z3 tile, a z6 tile, a map tile), keeping what it did; nothing new starts until you resume"
            m.addItem(it)
            let now = NSMenuItem(title: "Pause Building Now", action: #selector(pauseBuildNow), keyEquivalent: "")
            now.target = self
            now.isAlternate = true
            now.keyEquivalentModifierMask = [.option]
            now.toolTip = "Every Mac's running job frozen where it is at once; it goes on from there when you resume"
            m.addItem(now)
        }
        // This Mac alone paused, or going on (`scenic pause --here`, task #224): its jobs stop at
        // their next safe point and are given back, it takes no new work and hands the lead away;
        // the rest of the pool goes on.
        for h in holdItems(reply) {
            let it = NSMenuItem(title: h.title, action: h.held.map { $0 ? #selector(pauseHere) : #selector(resumeHere) }, keyEquivalent: "")
            it.target = self
            it.isEnabled = h.held != nil
            if !h.tip.isEmpty { it.toolTip = h.tip }
            m.addItem(it)
        }
        // A running job of this Mac's cancelled (`scenic job cancel`, crates/pipeline/src/control.rs
        // Cancel): stopped and given back, not failed, its work under way dropped. Option: held
        // until the lead runs a newer app.
        for j in runningJobs(serviceRunning: build.state.running) {
            guard let t = j.target else { continue }
            let it = NSMenuItem(title: "Cancel \(j.what)", action: #selector(cancelJob), keyEquivalent: "")
            it.target = self
            it.representedObject = [j.step, t, "rerun"]
            it.toolTip = "Stopped now and given back to the build (not a failure), its work under way dropped; it runs again from the start"
            m.addItem(it)
            let alt = NSMenuItem(title: "Cancel \(j.what), Rerun on a Newer App", action: #selector(cancelJob), keyEquivalent: "")
            alt.target = self
            alt.representedObject = [j.step, t, "update"]
            alt.isAlternate = true
            alt.keyEquivalentModifierMask = [.option]
            alt.toolTip = "Stopped and given back as with Cancel, and held until the lead runs a newer app than this one"
            m.addItem(alt)
        }
        // The pool's lead (docs/pool.md §11): handed over, asked for, taken over; asks to this
        // Mac's agent (crates/pipeline/src/agent/lead.rs), which checks each as its driver would.
        if let p = own?.pool, let v = p.lead {
            for li in leadItems(v, me: p.member) { m.addItem(leadMenuItem(li)) }
        }
        // This Mac's build caches, cleared on an ask to its agent (crates/pipeline/src/agent/
        // room.rs), which does it once the build is done, and says what it freed.
        if let c = cachesItem(own, now: Int(Date().timeIntervalSince1970), asked: clearAsked()) {
            let it = NSMenuItem(title: c.title, action: c.enabled ? #selector(clearCaches) : nil, keyEquivalent: "")
            it.target = self
            it.isEnabled = c.enabled
            if !c.tip.isEmpty { it.toolTip = c.tip }
            m.addItem(it)
        }
        // This Mac's disk room target (room.rs Target): its agent keeps that much free.
        if let r = roomItem(own, target: roomTarget(), free: diskFree(), disk: diskSize()) {
            let it = NSMenuItem(title: r.title, action: nil, keyEquivalent: "")
            it.toolTip = r.tip
            let sub = NSMenu()
            for c in r.choices {
                let ci = NSMenuItem(title: c.title, action: c.gb >= 0 ? #selector(setRoom) : nil, keyEquivalent: "")
                ci.target = self
                ci.tag = c.gb
                ci.state = c.on ? .on : .off
                if c.gb == 0 { sub.addItem(.separator()) }
                sub.addItem(ci)
            }
            it.submenu = sub
            m.addItem(it)
        }
        // The memory this Mac gives the build's jobs (buildmem.rs): its agent's asks are held to it.
        if let r = memoryItem(own, set: buildMemory(), totalMB: macMemoryMB()) {
            let it = NSMenuItem(title: r.title, action: nil, keyEquivalent: "")
            it.toolTip = r.tip
            let sub = NSMenu()
            for c in r.choices {
                let ci = NSMenuItem(title: c.title, action: c.gb >= 0 ? #selector(setMemory) : nil, keyEquivalent: "")
                ci.target = self
                ci.tag = c.gb
                ci.state = c.on ? .on : .off
                if c.gb == 0 { sub.addItem(.separator()) }
                sub.addItem(ci)
            }
            it.submenu = sub
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
        // The map's address for an iPhone or an iPad (this Mac's server writes it): pasted there
        // (Universal Clipboard), the map opens on the device.
        if let page = try? String(contentsOf: home.appendingPathComponent("map-page"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), !page.isEmpty {
            let it = NSMenuItem(title: "Copy the Map's Address", action: #selector(copyPage), keyEquivalent: "")
            it.target = self
            it.representedObject = page
            m.addItem(it)
        }
        // The build's page (its dashboard, and where a device helps): opened in the browser (the
        // one the panel shows, else the first address), or pasted on another device (Universal
        // Clipboard).
        do { let page = Panel.page
            let it = NSMenuItem(title: "Open the Build Page in the Browser", action: #selector(openPage), keyEquivalent: "")
            it.target = self
            it.representedObject = page
            m.addItem(it)
        }
        if let page = pageToShare() {
            let it = NSMenuItem(title: "Copy the Build Page's Address", action: #selector(copyPage), keyEquivalent: "")
            it.target = self
            it.representedObject = page
            m.addItem(it)
        }
        // The map server (a service of its own: started and stopped only from here or the window),
        // and quitting.
        if managing {
            m.addItem(.separator())
            if build.blocked != nil {
                let it = NSMenuItem(title: build.said, action: nil, keyEquivalent: "")
                it.isEnabled = false
                m.addItem(it)
            }
            if mapServer.installed {
                let it = mapServer.state.running
                    ? NSMenuItem(title: "Stop the Map Server…", action: #selector(stopServerItem), keyEquivalent: "")
                    : NSMenuItem(title: "Start the Map Server", action: #selector(startServerItem), keyEquivalent: "")
                it.target = self
                it.toolTip = mapServer.said
                m.addItem(it)
            }
            let win = NSMenuItem(title: "Show the Scenic Window", action: #selector(showWindow), keyEquivalent: "")
            win.target = self
            m.addItem(win)
            if build.quitting == "finish" {
                let jobs = runningJobs(serviceRunning: build.state.running)
                let info = NSMenuItem(title: "Quitting once this job ends" + (jobs.first?.left.map { ", about \(duration($0)) left" } ?? ""), action: nil, keyEquivalent: "")
                info.isEnabled = false
                m.addItem(info)
                let cancel = NSMenuItem(title: "Don't Quit", action: #selector(cancelQuit), keyEquivalent: "")
                cancel.target = self
                m.addItem(cancel)
                let now = NSMenuItem(title: "Stop Now and Quit", action: #selector(stopNowAndQuit), keyEquivalent: "")
                now.target = self
                m.addItem(now)
            } else {
                let quit = NSMenuItem(title: "Quit Scenic", action: #selector(quitApp), keyEquivalent: "")
                quit.target = self
                m.addItem(quit)
            }
        }
        return m
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification, withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound])
    }

    func leadMenuItem(_ li: LeadItem) -> NSMenuItem {
        let it = NSMenuItem(title: li.title, action: li.action != nil && li.enabled ? #selector(leadChosen) : nil, keyEquivalent: "")
        it.target = self
        it.isEnabled = li.enabled
        if !li.tip.isEmpty { it.toolTip = li.tip }
        it.representedObject = li.action.map { LeadBox($0) }
        if !li.children.isEmpty {
            let sub = NSMenu()
            sub.autoenablesItems = false
            for c in li.children { sub.addItem(leadMenuItem(c)) }
            it.submenu = sub
        }
        return it
    }

    /// A lead item chosen: confirmed when it needs it (a Mac away; a takeover, saying since when
    /// the lead's been gone and what it forces), then asked of this Mac's agent.
    @objc func leadChosen(_ sender: NSMenuItem) {
        guard let a = (sender.representedObject as? LeadBox)?.action else { return }
        switch a {
        case let .give(id, host, away):
            if away {
                let alert = NSAlert()
                alert.messageText = "Hand the build to \(host)?"
                alert.informativeText = "\(host) is away from home: the build's duties (planning, merging, publishing) run slowly over Tailscale until it's back."
                alert.addButton(withTitle: "Hand It Over")
                alert.addButton(withTitle: "Cancel")
                NSApp.activate()
                guard alert.runModal() == .alertFirstButtonReturn else { return }
            }
            askLead(["kind": "give", "to": id])
        case let .take(why, force, downgrade):
            let alert = NSAlert()
            alert.messageText = "Take over the build on this Mac?"
            var info = "No lead: \(why).\n\nThis Mac makes the next term naming itself and leads from the build's records on the NAS; nothing built is lost (the old lead's jobs hand off to the journal, and its leases lapse and go back out)."
            if let f = force { info += "\n\nForced: \(f)." }
            if let d = downgrade { info += "\n\nA downgrade: \(d) (its steps may build again what a newer app built)." }
            alert.informativeText = info
            alert.alertStyle = .warning
            alert.addButton(withTitle: "Take Over")
            alert.addButton(withTitle: "Cancel")
            NSApp.activate()
            guard alert.runModal() == .alertFirstButtonReturn else { return }
            askLead(["kind": "take", "force": force != nil, "downgrade": downgrade != nil])
        }
    }

    /// Asks this Mac's agent of the pool's lead: its ask file (crates/pipeline/src/control.rs
    /// LeadRequest), written whole, which the pool's next step takes up, within half a minute
    /// (crates/pipeline/src/agent/pool.rs Ticker).
    func askLead(_ ask: [String: Any]) {
        let name = SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "this Mac"
        let dir = home.appendingPathComponent("agent")
        let (tmp, dst) = (dir.appendingPathComponent("lead-request.json.menu.tmp"), dir.appendingPathComponent("lead-request.json"))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: ["ask": ask, "by": "the menu bar on \(name)", "at": Int(Date().timeIntervalSince1970)] as [String: Any]).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            post("Couldn't ask about the build's lead", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    /// Accepts a held unit's warnings (the item's unit), once its owner says so, naming them: an ask
    /// to this Mac's agent (crates/pipeline/src/inputs/mod.rs Ask, a file of its own in
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
            post("Couldn't accept \(unit)'s warnings", "\(error.localizedDescription)")
            return
        }
        poll()
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
        alert.addButton(withTitle: "Cancel the Job")
        alert.addButton(withTitle: "Keep It Running")
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
            post("Couldn't cancel the job", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    @objc func pauseBuild(_ sender: NSMenuItem) { ask("drain") }
    @objc func pauseBuildNow(_ sender: NSMenuItem) { ask("freeze") }
    @objc func resumeBuild(_ sender: NSMenuItem) { ask(nil) }
    @objc func pauseHere(_ sender: NSMenuItem) { askHold(true) }
    @objc func resumeHere(_ sender: NSMenuItem) { askHold(false) }

    /// Asks this Mac's agent to pause this Mac alone, or let it go on: its ask file
    /// (crates/pipeline/src/control.rs HoldRequest), written whole, taken up within seconds.
    func askHold(_ held: Bool) {
        let now = Int(Date().timeIntervalSince1970)
        let name = SCDynamicStoreCopyComputerName(nil, nil) as String? ?? "this Mac"
        let h: Any = held ? ["by": "the menu bar on \(name)", "at": now] as [String: Any] : NSNull()
        let dir = home.appendingPathComponent("agent")
        let (tmp, dst) = (dir.appendingPathComponent("hold-request.json.menu.tmp"), dir.appendingPathComponent("hold-request.json"))
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try JSONSerialization.data(withJSONObject: ["held": h, "at": now]).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            post("Couldn't ask this Mac to \(held ? "pause" : "go on")", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    /// Asks this Mac's agent to pause the build (`mode`: "drain" or "freeze") or let it go on (nil):
    /// its ask file (crates/pipeline/src/control.rs Request), written whole, which the agent takes up
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
            post("Couldn't ask the build to \(mode == nil ? "go on" : "pause")", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    /// Asks this Mac's agent to clear its build caches, once its owner says so (what goes, and how
    /// each comes back): its ask file (room.rs ClearRequest), written whole, which the agent takes
    /// up within seconds; signed with the name the agent goes by (its Mac's local host name).
    @objc func clearCaches(_ sender: NSMenuItem) {
        guard let c = own?.caches else { return }
        let alert = NSAlert()
        alert.messageText = "Clear this Mac's build caches (\(gb(c.clearable ?? 0)))?"
        let each = (c.each ?? []).map { "• \($0.what) \(gb($0.bytes)): \($0.back)" }.joined(separator: "\n")
        alert.informativeText = "\(each)\n\nKept: the Wikidata and Wikipedia answers (the NAS has them too; they free little), a pageview month the NAS lacks, and the map's offline copy."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Clear")
        alert.addButton(withTitle: "Cancel")
        NSApp.activate()
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = SCDynamicStoreCopyLocalHostName(nil) as String? ?? "this Mac"
        let dir = home.appendingPathComponent("agent")
        let (tmp, dst) = (dir.appendingPathComponent("\(clearFile).menu.tmp"), dir.appendingPathComponent(clearFile))
        do {
            try JSONSerialization.data(withJSONObject: ["by": "the menu bar on \(name)", "at": Int(Date().timeIntervalSince1970)] as [String: Any]).write(to: tmp)
            guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        } catch {
            post("Couldn't ask to clear the build's caches", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    /// Sets this Mac's disk room target to the item's GB (its tag; 0: off): the target's file in
    /// the agent's folder (room.rs Target), written whole, which the agent reads each loop; signed
    /// with the name the agent goes by.
    @objc func setRoom(_ sender: NSMenuItem) {
        let dir = home.appendingPathComponent("agent")
        let dst = dir.appendingPathComponent(targetFile)
        do {
            if sender.tag == 0 {
                if FileManager.default.fileExists(atPath: dst.path) { try FileManager.default.removeItem(at: dst) }
            } else {
                let name = SCDynamicStoreCopyLocalHostName(nil) as String? ?? "this Mac"
                let tmp = dir.appendingPathComponent("\(targetFile).menu.tmp")
                try JSONSerialization.data(withJSONObject: ["bytes": sender.tag << 30, "by": "the menu bar on \(name)", "at": Int(Date().timeIntervalSince1970)] as [String: Any]).write(to: tmp)
                guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
            }
        } catch {
            post("Couldn't set the disk room target", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    /// Sets the memory this Mac gives the build to the item's GB (its tag; 0: the memory guard's
    /// limit alone): the file in the agent's folder (buildmem.rs Memory), written whole, which the
    /// agent reads each loop; signed with the name the agent goes by.
    @objc func setMemory(_ sender: NSMenuItem) {
        let dir = home.appendingPathComponent("agent")
        let dst = dir.appendingPathComponent(memoryFile)
        do {
            if sender.tag == 0 {
                if FileManager.default.fileExists(atPath: dst.path) { try FileManager.default.removeItem(at: dst) }
            } else {
                let name = SCDynamicStoreCopyLocalHostName(nil) as String? ?? "this Mac"
                let tmp = dir.appendingPathComponent("\(memoryFile).menu.tmp")
                try JSONSerialization.data(withJSONObject: ["mb": sender.tag << 10, "by": "the menu bar on \(name)", "at": Int(Date().timeIntervalSince1970)] as [String: Any]).write(to: tmp)
                guard rename(tmp.path, dst.path) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
            }
        } catch {
            post("Couldn't set the build's memory", "\(error.localizedDescription)")
            return
        }
        poll()
    }

    @objc func copyPage(_ sender: NSMenuItem) {
        guard let page = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(page, forType: .string)
    }

    @objc func openLog(_ sender: NSMenuItem) {
        if let p = sender.representedObject as? String { NSWorkspace.shared.open(URL(fileURLWithPath: p)) }
    }

    @objc func openPage(_ sender: NSMenuItem) {
        guard let p = sender.representedObject as? String, let u = URL(string: p) else { return }
        panel.close()
        NSWorkspace.shared.open(u)
    }

    @objc func openMap(_ sender: NSMenuItem) {
        NSWorkspace.shared.open(server.deletingLastPathComponent())
    }

    // MARK: notifications, for every change

    func notifyChanges() {
        tellCaches()
        tellLead()
        guard let r = reply, let s = r.status else { return }
        let held = heldInputs(r)
        // (A unit held stays so for the notifications while it's checked again, its report gone
        // but the check not over: taken in only once its state is ok.)
        let rechecking = Set((s.inputs ?? []).filter { $0.state == "checking" }.map(\.unit)).intersection(seen?.held ?? [])
        let now = Seen(job: s.firstJob?.id, paused: s.firstJob?.paused != nil, lastEnded: s.recent.map(\.ended).max() ?? 0, outOfTouch: r.now - s.beat > outOfTouch, buildPaused: s.pause != nil, held: Set(held.map(\.unit)).union(rechecking))
        defer { seen = now }
        // The first answer only sets what changes are measured from.
        guard let was = seen else { return }
        // The gate: a unit held, or taken in (its held change accepted or fixed, its check over).
        for v in held where !was.held.contains(v.unit) {
            post("Input held: \(v.unit)", "\(inputLine(v)). The map goes on with the last good version.")
        }
        for u in was.held.subtracting(now.held).sorted() {
            post("Input taken in: \(u)", "Its held change passed the gate")
        }
        if now.buildPaused != was.buildPaused {
            post(now.buildPaused ? "Build paused" : "Build going on", s.pause.map { "From \($0.by)" } ?? "Picking up where it stopped")
        }
        if now.outOfTouch != was.outOfTouch {
            post(now.outOfTouch ? "Lead out of touch" : "Lead back", now.outOfTouch ? "Not heard from since \(clock(s.beat))" : s.host)
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
        if let j = s.firstJob {
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

    /// The pool's lead changing, and this Mac's ask of it ending, each told once (the first look
    /// only sets where they're told from).
    var seenLead: (term: Int, asked: Int, alert: Bool)?
    func tellLead() {
        guard let v = own?.pool?.lead else { return }
        let asked = v.asked.map { ["done", "failed", "refused"].contains($0.state) ? $0.since : 0 } ?? 0
        defer { seenLead = (v.term, asked, v.alert != nil) }
        // (The alert told on the first look too: a menu bar started while it's on says it once.)
        guard let was = seenLead else {
            if let a = v.alert { post("The build has no lead", a) }
            return
        }
        if v.term > was.term, let l = v.lead {
            post("\(l.host) leads the build", "Term \(l.term): \(l.how)")
        }
        if !was.alert, let a = v.alert {
            post("The build has no lead", a)
        }
        if asked > was.asked, let a = v.asked {
            post(a.state == "done" ? "Lead: done" : "Lead: \(a.state == "refused" ? "refused" : "came to nothing")", a.said)
        }
    }

    /// This Mac's caches cleared, or not cleared and why, each told once
    /// (the first look at its agent's status only sets where they're told from).
    func tellCaches() {
        guard let c = own?.caches else { return }
        let now = (cleared: c.cleared?.at ?? 0, declined: c.declined?.at ?? 0)
        defer { seenCaches = now }
        guard let was = seenCaches else { return }
        if let f = c.cleared, f.at != was.cleared {
            post("Freed \(gb(f.bytes))", "This Mac's build caches: \(freedText(f)). Later jobs copy back from the NAS what they need.")
        }
        if let f = c.declined, f.at != was.declined {
            post("Build caches not cleared", f.why_not ?? "")
        }
    }

}

/// "17:04:05.123", for --menu-proof's log.
func stamp() -> String {
    let f = DateFormatter()
    f.dateFormat = "HH:mm:ss.SSS"
    return f.string(from: Date())
}

let usage = """
usage: scenic-status [option]
  (none)                     the app: the menu bar item, the build service and the map server's window
  --no-window                the app, its window not opened (an update's reopen)
  --plan                     whether opening it would manage the build service, or why not
  --services                 the build service's and the map server's state
  --unregister               the build service and the app off the login items
  --print                    the icon, the status's lines and the menu's items now, as text
  --replay a.json b.json …   the notifications a sequence of answers would send (each file
                             /api/build's answer, with this Mac's agent's own status beside it as
                             "own", and "clear_asked" while an ask to clear its caches waits), and
                             the caches' item
  --render lines.png         the status's lines drawn as they lay out (dark), for checking
  --show panel|menu          runs, and opens the panel (closed 20 s later) or the menu
  --menu-proof               runs, its polls and notifications printed with the time, a menu held
                             open 25 s between: they go on while it's open
  --reopen <pid> <app>       an update's helper: opens <app> once <pid> has ended
  --test <dir>               its self-tests: arguments, the reopen helper, launchd's state read from
                             captured launchctl output (tools/status/test)
  --help, -h                 this
"""

/// The options and how many values each takes (-1: all the rest, at least one).
let options: [String: Int] = [
    "--no-window": 0, "--plan": 0, "--services": 0, "--unregister": 0, "--print": 0, "--replay": -1,
    "--render": 1, "--show": 1, "--menu-proof": 0, "--reopen": 2, "--test": 1,
]

/// The options that run something other than the app: one at a time, and not with the app's own
/// (--no-window, --show, --menu-proof).
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
    // The build service's and the map server's state, as the app sees them.
    let b = BuildService()
    b.refresh()
    let m = MapServer()
    m.refresh()
    print("\(b.said) [\(b.service.status.rawValue)]")
    print(m.said)
} else if args.contains("--print") {
    // The status now, as the icon and menu would show it.
    let done = DispatchSemaphore(value: 0)
    var r: Reply?
    URLSession.shared.dataTask(with: server.appendingPathComponent("api/build")) { data, _, _ in
        r = data.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) }
        done.signal()
    }.resume()
    done.wait()
    let (kind, line) = state(r, own: ownStatus())
    print("icon: \(kind.symbol)\(heldInputs(r).isEmpty ? "" : " with a warning badge")")
    for v in heldInputs(r) { print("item: \(inputLine(v))") }
    let own = ownStatus()
    for l in lines(r, line, own: own) {
        let bar = l.style == .bar ? "[" + String(repeating: "█", count: Int(l.fraction * 20)) + String(repeating: "░", count: 20 - Int(l.fraction * 20)) + "] " : ""
        print(l.style == .separator ? "────" : (l.style == .title ? "" : "  ") + bar + l.text)
    }
    for h in holdItems(r) { print("item: \(h.title)\(h.held == nil ? " (disabled)" : "")") }
    if let c = cachesItem(own, now: Int(Date().timeIntervalSince1970), asked: clearAsked()) {
        print("item: \(c.title)\(c.enabled || c.tip.isEmpty ? "" : " (disabled: \(c.tip))")")
    }
    if let r = roomItem(own, target: roomTarget(), free: diskFree(), disk: diskSize()) {
        print("item: \(r.title) [\(r.choices.map { ($0.on ? "✓" : "") + $0.title }.joined(separator: " | "))]")
    }
    if let r = memoryItem(own, set: buildMemory(), totalMB: macMemoryMB()) {
        print("item: \(r.title) [\(r.choices.map { ($0.on ? "✓" : "") + $0.title }.joined(separator: " | "))]")
    }
    if let p = own?.pool, let v = p.lead {
        for li in leadItems(v, me: p.member) {
            print("item: \(li.title)\(li.enabled ? "" : " (disabled\(li.tip.isEmpty ? "" : ": \(li.tip)"))")\(li.enabled && !li.tip.isEmpty ? " — \(li.tip)" : "")")
            for c in li.children { print("    ▸ \(c.title)\(c.enabled ? "" : " (disabled: \(c.tip))")") }
        }
    }
} else if let i = args.firstIndex(of: "--render") {
    // The menu bar's icon and the menu's lines as views, stacked as the menu stacks them, drawn into a PNG.
    let done = DispatchSemaphore(value: 0)
    var r: Reply?
    URLSession.shared.dataTask(with: server.appendingPathComponent("api/build")) { data, _, _ in
        r = data.flatMap { try? JSONDecoder().decode(Reply.self, from: $0) }
        done.signal()
    }.resume()
    done.wait()
    _ = NSApplication.shared
    let (kind, line) = state(r, own: ownStatus())
    // (The menu bar's icon first, as the bar draws it: a template image, light on the dark bar.)
    let bar = NSImageView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: 24))
    bar.image = iconImage(kind, held: !heldInputs(r).isEmpty, line: line)
    bar.contentTintColor = .white
    bar.imageAlignment = .alignLeft
    let views: [NSView] = [bar] + lines(r, line, own: ownStatus()).map { l in
        l.style == .separator ? NSView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: 11)) : l.style == .bar ? BarView(l.text, fraction: l.fraction) : LineView(l.text, font: fontFor(l.style).0, color: fontFor(l.style).1, wrapAnywhere: l.style == .mono)
    } + (ownStatus()?.pool.flatMap { p in p.lead.map { leadItems($0, me: p.member) } } ?? []).flatMap { li in
        [LineView(li.title + (li.children.isEmpty ? "" : "  ▸"), font: .menuFont(ofSize: 0), color: li.enabled ? .labelColor : .tertiaryLabelColor, wrapAnywhere: false)]
            + li.children.map { c in LineView("      \(c.title)\(c.enabled ? "" : " — \(c.tip)")", font: .menuFont(ofSize: NSFont.smallSystemFontSize), color: c.enabled ? .labelColor : .tertiaryLabelColor, wrapAnywhere: false) }
    } + holdItems(r).map { hi in LineView(hi.title, font: .menuFont(ofSize: 0), color: hi.held != nil ? .labelColor : .tertiaryLabelColor, wrapAnywhere: false) }
        + (r?.log != nil ? ["Open the Build Log"] : []).map { LineView($0, font: .menuFont(ofSize: 0), color: .labelColor, wrapAnywhere: false) }
        + [LineView("Open the Map", font: .menuFont(ofSize: 0), color: .labelColor, wrapAnywhere: false)]
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
    // The notifications a sequence of answers would send.
    let d = AppDelegate()
    d.sink = { title, body in print("notify: \(title) — \(body)") }
    d.sinkIsCenter = false
    for f in args[(i + 1)...] {
        let data = (try? Data(contentsOf: URL(fileURLWithPath: f))) ?? Data()
        d.reply = try? JSONDecoder().decode(Reply.self, from: data)
        let beside = try? JSONDecoder().decode(Replay.self, from: data)
        d.own = beside?.own
        print("\(f): \(state(d.reply, own: d.own).1)\(heldInputs(d.reply).isEmpty ? "" : " (badged)")")
        for v in heldInputs(d.reply) { print("  item: \(inputLine(v))") }
        if let c = cachesItem(d.own, now: d.reply?.now ?? Int(Date().timeIntervalSince1970), asked: beside?.clear_asked ?? false) {
            print("  item: \(c.title)\(c.enabled || c.tip.isEmpty ? "" : " (disabled: \(c.tip))")")
            // (What its confirmation lists.)
            for g in (c.enabled ? d.own?.caches?.each : nil) ?? [] { print("    \(g.what) \(gb(g.bytes)): \(g.back)") }
        }
        // (The target and free space as the agent's status says them.)
        if let r = roomItem(d.own, target: d.own?.caches?.room?.target?.size, free: d.own?.caches?.room?.free, disk: nil) {
            print("  item: \(r.title) [\(r.choices.map { ($0.on ? "✓" : "") + $0.title }.joined(separator: " | "))]")
            print("    tip: \(r.tip)")
        }
        if let p = d.own?.pool, let v = p.lead {
            for li in leadItems(v, me: p.member) {
                print("  item: \(li.title)\(li.enabled ? "" : " (disabled: \(li.tip))")")
                for c in li.children { print("    ▸ \(c.title)\(c.enabled ? "" : " (disabled: \(c.tip))")") }
            }
        }
        d.notifyChanges()
    }
} else {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    delegate.proof = args.contains("--menu-proof")
    if let i = args.firstIndex(of: "--show") { delegate.showAtStart = args[i + 1] }
    app.delegate = delegate
    // (Never in the Dock (Info.plist's LSUIElement too): the menu bar item and the app's window.)
    app.setActivationPolicy(.accessory)
    app.run()
}
