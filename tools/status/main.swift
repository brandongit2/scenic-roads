// Scenic's menu bar item: the build agent's state at a glance (docs/plan.md §8, Status). An icon for
// the state (building, paused, waiting, nothing to do, a problem, the build Mac out of touch), a
// menu with the details, and a notification for every change. It asks the map's server on this Mac
// (`/api/build`), which answers with this Mac's own agent's status when the agent runs here and
// with the heartbeat the agent copies to the NAS otherwise. This Mac's own agent's status, read
// from its file, says what its build caches hold, whether they can be cleared now,
// and the last trim and clear (crates/pipeline/src/agent/room.rs): Clear the Build's Caches asks
// that agent to clear them. Disk Room shows the disk's free space and the owner's room target, and
// sets it (a few presets, or Off): the free space that agent keeps, freeing its caches to it.
//
// The launcher runs it (`scenic-launcher status`, from ~/Library/LaunchAgents/local.scenic.status.plist)
// from the installed app; it quits when a newer app is installed, and the launcher starts that one.
//
//   swiftc -O -swift-version 5 -o Scenic.app/Contents/MacOS/scenic-status tools/status/main.swift
//   scenic-status --print                   the icon and menu for the status now, as text
//   scenic-status --replay a.json b.json …  the notifications a sequence of answers would send (each
//                                           file /api/build's answer, with this Mac's agent's
//                                           own status beside it as "own", and "clear_asked"
//                                           while an ask to clear its caches waits), and the
//                                           caches' item
//   scenic-status --render menu.png          the menu's lines drawn as they lay out (dark), for checking
//   scenic-status --wait-replaced           waits, without a window, until a newer app is installed
// SCENIC_STATUS_SERVER overrides the server (http://127.0.0.1:8080), SCENIC_HOME the app folder.

import AppKit
import CoreServices
import SystemConfiguration
import UserNotifications

let server = URL(string: ProcessInfo.processInfo.environment["SCENIC_STATUS_SERVER"] ?? "http://127.0.0.1:8080")!
let home = ProcessInfo.processInfo.environment["SCENIC_HOME"].map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/scenic")
/// Without a heartbeat for this long, the build Mac counts as out of touch (asleep, off, away, or
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
}

/// This Mac's downloads (crates/server/src/downloads.rs `summary`).
struct Offline: Decodable {
    let world: Bool
    let areas: Int
    let bytes: Int
    let here: Int
    /// The NAS is reachable from this Mac.
    let nas: Bool
}

/// The menu's line on what this Mac can show without the NAS.
func offlineText(_ o: Offline) -> String {
    if !o.world {
        return o.nas ? "Nothing downloaded: the map needs the NAS to show anything (Settings → Regions)" : "Away from the NAS with nothing downloaded: the map can't show anything"
    }
    var t = "Downloaded: the World, zoomed out"
    if o.areas > 0 { t += " and \(o.areas) area\(o.areas == 1 ? "" : "s")" }
    if o.here < o.bytes { t += o.nas ? " · \(min(99, o.here * 100 / max(1, o.bytes))) % here" : " · \(min(99, o.here * 100 / max(1, o.bytes))) % here, away from the NAS" }
    return t
}

struct Status: Decodable {
    let host: String
    let app: String?
    let beat: Int
    let conditions: Conditions
    let job: Job?
    /// Its second job, beside the first (agents from 2026-10-06 on).
    let beside: Job?
    let waiting: [Waiting]
    let recent: [Done]
    let built: [String: Built]?
    /// The build to the end (agents from 2026-10-03 on).
    let checklist: [Step]?
    /// Other Macs helping, building units (agents from 2026-10-04 on).
    let helpers: [Helper]?
    /// Every worker the coordinator heard from lately: helpers and web pages (agents from 2026-10-05 on).
    let workers: [Worker]?
    /// The build's pause, while it's paused (agents from 2026-10-05 on).
    let pause: PauseInfo?
    /// The regions (their names), and when the build will be done and the map next updated (the
    /// forecast: crates/pipeline/src/agent/forecast.rs; agents from 2026-10-05 on).
    let regions: [Recipe]?
    let forecast: Forecast?
}

/// A --replay file's this Mac's agent's own status and whether an ask to clear its caches waits,
/// beside its answer.
struct Replay: Decodable {
    let own: Own?
    let clear_asked: Bool?
}

/// This Mac's own agent's status, as it writes it in its folder (the build Mac's `status.json`, a
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
    let handing: Handing?
    let offer: PoolOffer?
    let auto: Bool?
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

struct PoolOffer: Decodable {
    let to: String
    let host: String
    let why: String
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
/// with no lead in touch, "Take Over the Build…"; the proactive offer on every Mac; an ask under way.
func leadItems(_ v: PoolView, me: String) -> [LeadItem] {
    var out: [LeadItem] = []
    if let a = v.asked, a.state == "passed" || a.state == "going" {
        out.append(LeadItem(title: "\(a.said)…", enabled: false))
    }
    if let o = v.offer {
        out.append(LeadItem(title: "Hand the Build to \(o.host)", tip: "\(o.why), and \(o.host) is home on power\((v.auto ?? false) ? "; handed over by itself after five minutes" : "")", action: .give(o.to, o.host, false)))
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
/// can), and the last trim after the build, the last clear done and the last ask declined.
struct Caches: Decodable {
    let clearable: Int?
    let each: [Gone]?
    let why_not: String?
    let trimmed: Freed?
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

/// What a trim or a clear did (room.rs Freed): when, the bytes freed by cache, what stays, and for
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

/// The forecast's part the menu shows: when it'll all be done (and the range), and the rounds of
/// publishing to come, each with the regions it adds.
struct Forecast: Decodable {
    let done_at: Int?
    let range: [Int]?
    let rounds: [Round]?
    /// Why there's no finish to forecast (nothing left; a new pass first; the units waiting).
    let why: String?
}

struct Round: Decodable {
    let at: Int
    let regions: [String]
    let last: Bool
}

/// The build's pause (crates/pipeline/src/control.rs Pause): at the next safe point ("drain") or
/// at once ("freeze"), who asked, since when.
struct PauseInfo: Decodable {
    let mode: String
    let by: String
    let at: Int
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

/// A helper on another Mac, as the main agent last read its status (`here`: this Mac's own, fresh).
struct Helper: Decodable {
    let host: String
    let beat: Int
    let job: Job?
    let pause: PauseInfo?
    let here: Bool?
}

/// A step of the build to the end: done of total (total unknown until an earlier step makes it), or
/// for a group of single jobs how many are left; its jobs left by name (`next`, the first the one
/// under way), and why it waits or which Mac is on it (`note`).
struct Step: Decodable {
    let what: String
    let steps: [String]
    let done: Int?
    let total: Int?
    let left: Int?
    let unit: String?
    let next: [String]?
    let note: String?
    /// Which of its jobs a helper may do: "all", or the parts by name.
    let shared: String?

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

/// A time to come: "16:40" today, "Tue 07:50" within the week, else "8 Oct 07:50".
func soon(_ t: Int) -> String {
    let d = Date(timeIntervalSince1970: TimeInterval(t))
    let f = DateFormatter()
    f.dateFormat = Calendar.current.isDateInToday(d) ? "HH:mm" : abs(d.timeIntervalSinceNow) < 6 * 86400 ? "EEE HH:mm" : "d MMM HH:mm"
    return f.string(from: d)
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
    // Paused: stopping (a job finishing what it's on, here or on a helper), else paused.
    if let p = s.pause {
        let stopping = (s.job.map { $0.paused == nil } ?? false) || (s.beside.map { $0.paused == nil } ?? false) || helping.contains { $0.job?.pausing != nil }
        return (.paused, stopping ? "Pausing: finishing what it's on" : "Paused since \(clock(p.at))")
    }
    // (The build Mac's jobs: its second's beside its first.)
    if let j = s.job ?? s.beside {
        if j.paused != nil && (s.beside.map { $0.paused != nil } ?? true) {
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

/// A job's progress in words: "40% · 2.4 of 6 areas · about 12 min left" (the item under way
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
        if let next = (f.rounds ?? []).first(where: { !$0.regions.isEmpty }) {
            let names = Dictionary((s.regions ?? []).map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
            let which = next.regions.prefix(3).map { names[$0] ?? $0 }.joined(separator: ", ") + (next.regions.count > 3 ? " and \(next.regions.count - 3) more" : "")
            out.append(Line(text: "Next map update ≈ \(soon(next.at)): \(which)", style: .small))
        }
    }
    if let p = s.pause {
        let how = p.mode == "freeze" ? "every Mac's job frozen where it was" : "every Mac's job stops at its next safe point"
        out.append(Line(text: "From \(p.by), \(clock(p.at)): \(how); nothing new starts until it's resumed", style: .small))
    }
    if let j = s.job {
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
        out.append(Line(text: "Running \(duration(r.now - j.started)) (since \(clock(j.started)))", style: .small))
        // The log's last lines, without the terminal's colour codes.
        let plain = (j.tail ?? "").replacingOccurrences(of: "\u{1B}\\[[0-9;]*[A-Za-z]", with: "", options: .regularExpression)
        for l in plain.split(separator: "\n").suffix(3) where !l.trimmingCharacters(in: .whitespaces).isEmpty {
            out.append(Line(text: tildes(String(l).trimmingCharacters(in: .whitespaces)), style: .mono))
        }
    }
    // The build Mac's second job, beside the first.
    if let j = s.beside {
        out.append(Line(text: "Beside it: \(j.what)", style: .plain))
        if let p = j.progress {
            out.append(Line(text: progressText(p, paused: j.paused != nil, now: r.now), style: .bar, fraction: p.total > 0 ? min(1, p.done / p.total) : 0))
        }
        if let p = j.paused { out.append(Line(text: p, style: .small)) } else if j.pausing != nil { out.append(Line(text: "Stopping at its next safe point (what it's on is kept)", style: .small)) }
        out.append(Line(text: "Running \(duration(r.now - j.started)) (since \(clock(j.started)))", style: .small))
    }
    // The other Macs helping (units only), each with its job.
    for h in freshHelpers(r) {
        guard let j = h.job else {
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
    // This Mac's build caches: what a clear would free, and the last trim after the build and clear.
    if let c = own?.caches {
        var t = "Build caches here: \(c.clearable.map(gb) ?? "not counted yet")"
        if let f = c.trimmed, f.why_not == nil { t += " · trimmed \(clock(f.at)), \(gb(f.bytes)) freed" }
        if let f = c.cleared, f.why_not == nil { t += " · cleared \(clock(f.at)), \(gb(f.bytes)) freed" }
        out.append(Line(text: t, style: .small))
    }
    // The pool: who leads, each member (this Mac's agent's view).
    if let v = own?.pool?.lead { out += poolLines(v, now: r.now) }
    // The build to the end: each step done, under way, or to come.
    if let steps = s.checklist, !steps.isEmpty {
        let now = [s.job, s.beside].compactMap { $0.map { String($0.id.split(separator: " ").first ?? "") } }
        let finished = steps.filter(\.finished).count
        out.append(Line(text: "", style: .separator))
        let legend = steps.contains { !$0.finished && $0.shared != nil } ? "  ·  ⇄ helpers can take part" : ""
        out.append(Line(text: "To the end: \(finished) of \(steps.count) steps done\(legend)", style: .header))
        for st in steps {
            if st.finished {
                out.append(Line(text: "✓ \(st.what)", style: .stepDone))
                continue
            }
            var count = ""
            if let t = st.total, let d = st.done {
                count = ": \(grouped(Double(d))) of \(grouped(Double(t))) \(st.unit ?? "")"
            } else if let l = st.left, l > 0 {
                count = ": \(l) left"
            }
            let running = now.contains { st.steps.contains($0) }
            let mark = st.shared != nil ? "  ⇄" : ""
            out.append(Line(text: "\(running ? "▸" : "○") \(st.what)\(count)\(mark)", style: running ? .stepNow : .stepToDo))
            // (Only some of its jobs: which.)
            if let sh = st.shared, sh != "all" { out.append(Line(text: "      ⇄ helpers can take its \(sh)", style: .small)) }
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

/// A lead item's action, carried by its menu item.
final class LeadBox: NSObject {
    let action: LeadAction
    init(_ a: LeadAction) { action = a }
}

/// What notifications compare: the job, whether it's paused, the last finished job, out of touch.
struct Seen {
    var job: String?
    var paused: Bool
    var lastEnded: Int
    var outOfTouch: Bool
    var buildPaused: Bool
}

/// This Mac's ask to its agent (crates/pipeline/src/control.rs), while it waits to be taken up: to
/// pause (true) or go on (false).
let askFile = "pause-request.json"
func pendingAsk() -> Bool? {
    guard let d = try? Data(contentsOf: home.appendingPathComponent("agent").appendingPathComponent(askFile)),
          let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return nil }
    return !(o["pause"] is NSNull || o["pause"] == nil)
}

/// This Mac's own agent's status (`Own`): the build Mac's or a helper's file, the fresher.
func ownStatus() -> Own? {
    let dir = home.appendingPathComponent("agent")
    return ["status.json", "helper.json"].compactMap { (try? Data(contentsOf: dir.appendingPathComponent($0))).flatMap { try? JSONDecoder().decode(Own.self, from: $0) } }.max { $0.beat < $1.beat }
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
    var title = "Disk Room: \(free.map(gb) ?? "?") free"
    title += target.map { " · target \(gb($0))" } ?? " · no target"
    let r = own?.caches?.room
    var tip = "The free space this Mac's agent keeps: it frees its build caches to it, as far as needed, and starts no job that would cross it, until it's lowered or off."
    if let t = target, r?.target?.size == t, let why = r?.short { tip = "Short of it: \(why)" }
    if let t = target, r?.target?.size == t, let f = r?.toward, f.bytes > 0 { tip += "\nFreed toward it \(clock(f.at)): \(freedText(f))" }
    var choices = roomPresets.filter { p in disk.map { p << 30 < $0 } ?? true }.map { (title: "Keep \($0) GB Free", gb: $0, on: target == $0 << 30) }
    if let t = target, !choices.contains(where: \.on) { choices.append((title: "Keep \(gb(t)) Free", gb: -1, on: true)) }
    choices.append((title: "Off", gb: 0, on: target == nil))
    return (title, tip, choices)
}

/// "51.2 GB", or under a GB, "350 MB" (number and unit never split across lines).
func gb(_ b: Int) -> String {
    b >= 1 << 30 ? String(format: "%.1f\u{00A0}GB", Double(b) / Double(1 << 30)) : "\(b >> 20)\u{00A0}MB"
}

/// What a trim or a clear freed, by cache, the most first: "canopy squares 45.0 GB, raw terrain
/// tiles 24.1 GB", and what stays.
func freedText(_ f: Freed) -> String {
    let words = ["canopy": "canopy squares", "terrain": "raw terrain tiles", "blobs": "copies of the records' files", "base": "base packs", "dem": "the DEM seed", "copies": "copies of the NAS's files", "heritage": "the heritage jobs' planet clip"]
    var t = (f.freed ?? [:]).filter { $0.value > 0 }.sorted { $0.value > $1.value }.map { "\(words[$0.key] ?? $0.key) \(gb($0.value))" }.joined(separator: ", ")
    if t.isEmpty { t = "nothing" }
    if let l = f.left, l > 0 { t += "; \(gb(l)) kept (the NAS hasn't it yet)" }
    return t
}

/// The menu's item for this Mac's build caches (none without an agent here): Clear the Build's
/// Caches with what it would free, enabled once the build is done and no job runs here (why not, in
/// its tooltip); while an ask waits, that it's clearing.
func cachesItem(_ own: Own?, now: Int, asked: Bool) -> (title: String, enabled: Bool, tip: String)? {
    guard let own = own, let c = own.caches else { return nil }
    if asked { return ("Clearing the Build's Caches… (asked; this Mac's agent does it between jobs)", false, "") }
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
    /// This Mac's agent's own status (its caches), read with each answer, and its last trim's and
    /// clear's times as last told.
    var own: Own?
    var seenCaches: (trimmed: Int, cleared: Int, declined: Int)?
    var polling = false
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
        // Known to Launch Services by its bundle (notifications need it): the launcher starts the
        // executable inside it directly.
        LSRegisterURL(Bundle.main.bundleURL as CFURL, true)
        _ = launchedFrom
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        // (A device's asks to help, from before pages helped with no key: taken away.)
        center.getDeliveredNotifications { ns in
            let asks = ns.map(\.request.identifier).filter { $0.hasPrefix("ask-") }
            if !asks.isEmpty { center.removeDeliveredNotifications(withIdentifiers: asks) }
        }
        center.requestAuthorization(options: [.alert, .sound]) { _, _ in }
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
                self.own = ownStatus()
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
        for l in lines(reply, line, own: own) {
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
        // The whole build paused (every Mac), or going on: an ask to this Mac's agent, which passes it
        // on to the build Mac (crates/pipeline/src/control.rs). Option: at once, frozen where it is.
        // (Paused as this Mac knows it: the build Mac's, or, a helper's own, its own.)
        let ownHelper = reply?.status?.helpers?.first { $0.here == true }
        let pausedHere = reply?.status?.pause != nil || ownHelper?.pause != nil
        let asked = pendingAsk()
        if let a = asked {
            let it = NSMenuItem(title: a ? "Pausing… (asked; the build Mac takes it up within seconds)" : "Resuming… (asked)", action: nil, keyEquivalent: "")
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
            it.toolTip = "Every Mac's running job stops at its next safe point (an area, a map tile), keeping what it did; nothing new starts until you resume"
            m.addItem(it)
            let now = NSMenuItem(title: "Pause Building Now", action: #selector(pauseBuildNow), keyEquivalent: "")
            now.target = self
            now.isAlternate = true
            now.keyEquivalentModifierMask = [.option]
            now.toolTip = "Every Mac's running job frozen where it is at once; it goes on from there when you resume"
            m.addItem(now)
        }
        // The pool's lead (docs/pool.md §11): handed over, asked for, taken over; asks to this
        // Mac's agent (crates/pipeline/src/agent/lead.rs), which checks each as its driver would.
        if let p = own?.pool, let v = p.lead {
            for li in leadItems(v, me: p.member) { m.addItem(leadMenuItem(li)) }
        }
        // This Mac's build caches, cleared on an ask to its agent (crates/pipeline/src/agent/
        // room.rs), which does it between jobs once the build is done, and says what it freed.
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
        // The build's page (its dashboard, and where a device helps), which the build Mac's agent
        // writes: pasted on another device (Universal Clipboard).
        if let page = try? String(contentsOf: home.appendingPathComponent("agent/coord/page"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines), !page.isEmpty {
            let it = NSMenuItem(title: "Copy the Build Page's Address", action: #selector(copyPage), keyEquivalent: "")
            it.target = self
            it.representedObject = page
            m.addItem(it)
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
    /// LeadRequest), written whole, which the agent takes up within seconds.
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

    @objc func pauseBuild(_ sender: NSMenuItem) { ask("drain") }
    @objc func pauseBuildNow(_ sender: NSMenuItem) { ask("freeze") }
    @objc func resumeBuild(_ sender: NSMenuItem) { ask(nil) }

    /// Asks this Mac's agent to pause the build (`mode`: "drain" or "freeze") or let it go on (nil):
    /// its ask file (crates/pipeline/src/control.rs Request), written whole, which the agent takes up
    /// within seconds and passes on to the build Mac.
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
        tellCaches()
        tellLead()
        guard let r = reply, let s = r.status else { return }
        let now = Seen(job: s.job?.id, paused: s.job?.paused != nil, lastEnded: s.recent.map(\.ended).max() ?? 0, outOfTouch: r.now - s.beat > outOfTouch, buildPaused: s.pause != nil)
        defer { seen = now }
        // The first answer only sets what changes are measured from.
        guard let was = seen else { return }
        if now.buildPaused != was.buildPaused {
            post(now.buildPaused ? "Build paused" : "Build going on", s.pause.map { "From \($0.by)" } ?? "Picking up where it stopped")
        }
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

    /// The pool's lead changing, and this Mac's ask of it ending, each told once (the first look
    /// only sets where they're told from).
    var seenLead: (term: Int, asked: Int)?
    func tellLead() {
        guard let v = own?.pool?.lead else { return }
        let asked = v.asked.map { ["done", "failed", "refused"].contains($0.state) ? $0.since : 0 } ?? 0
        defer { seenLead = (v.term, asked) }
        guard let was = seenLead else { return }
        if v.term > was.term, let l = v.lead {
            post("\(l.host) leads the build", "Term \(l.term): \(l.how)")
        }
        if asked > was.asked, let a = v.asked {
            post(a.state == "done" ? "Lead: done" : "Lead: \(a.state == "refused" ? "refused" : "came to nothing")", a.said)
        }
    }

    /// This Mac's caches trimmed after the build, cleared, or not cleared and why, each told once
    /// (the first look at its agent's status only sets where they're told from).
    func tellCaches() {
        guard let c = own?.caches else { return }
        let now = (trimmed: c.trimmed?.at ?? 0, cleared: c.cleared?.at ?? 0, declined: c.declined?.at ?? 0)
        defer { seenCaches = now }
        guard let was = seenCaches else { return }
        if let f = c.cleared, f.at != was.cleared {
            post("Freed \(gb(f.bytes))", "This Mac's build caches: \(freedText(f)). Later jobs copy back from the NAS what they need.")
        }
        if let f = c.declined, f.at != was.declined {
            post("Build caches not cleared", f.why_not ?? "")
        }
        // (Not one with nothing to do, the caches empty already.)
        if let f = c.trimmed, f.at != was.trimmed, f.bytes >= 50 << 20 {
            post("Build caches trimmed", "Freed \(gb(f.bytes)) on this Mac now that the build is done: \(freedText(f))")
        }
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
    let own = ownStatus()
    for l in lines(r, line, own: own) {
        let bar = l.style == .bar ? "[" + String(repeating: "█", count: Int(l.fraction * 20)) + String(repeating: "░", count: 20 - Int(l.fraction * 20)) + "] " : ""
        print(l.style == .separator ? "────" : (l.style == .title ? "" : "  ") + bar + l.text)
    }
    if let c = cachesItem(own, now: Int(Date().timeIntervalSince1970), asked: clearAsked()) {
        print("item: \(c.title)\(c.enabled || c.tip.isEmpty ? "" : " (disabled: \(c.tip))")")
    }
    if let r = roomItem(own, target: roomTarget(), free: diskFree(), disk: diskSize()) {
        print("item: \(r.title) [\(r.choices.map { ($0.on ? "✓" : "") + $0.title }.joined(separator: " | "))]")
    }
    if let p = own?.pool, let v = p.lead {
        for li in leadItems(v, me: p.member) {
            print("item: \(li.title)\(li.enabled ? "" : " (disabled\(li.tip.isEmpty ? "" : ": \(li.tip)"))")\(li.enabled && !li.tip.isEmpty ? " — \(li.tip)" : "")")
            for c in li.children { print("    ▸ \(c.title)\(c.enabled ? "" : " (disabled: \(c.tip))")") }
        }
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
    let views: [NSView] = lines(r, line, own: ownStatus()).map { l in
        l.style == .separator ? NSView(frame: NSRect(x: 0, y: 0, width: LineView.width, height: 11)) : l.style == .bar ? BarView(l.text, fraction: l.fraction) : LineView(l.text, font: fontFor(l.style).0, color: fontFor(l.style).1, wrapAnywhere: l.style == .mono)
    } + (ownStatus()?.pool.flatMap { p in p.lead.map { leadItems($0, me: p.member) } } ?? []).flatMap { li in
        [LineView(li.title + (li.children.isEmpty ? "" : "  ▸"), font: .menuFont(ofSize: 0), color: li.enabled ? .labelColor : .tertiaryLabelColor, wrapAnywhere: false)]
            + li.children.map { c in LineView("      \(c.title)\(c.enabled ? "" : " — \(c.tip)")", font: .menuFont(ofSize: NSFont.smallSystemFontSize), color: c.enabled ? .labelColor : .tertiaryLabelColor, wrapAnywhere: false) }
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
    d.sinkIsCenter = false
    for f in args[(i + 1)...] {
        let data = (try? Data(contentsOf: URL(fileURLWithPath: f))) ?? Data()
        d.reply = try? JSONDecoder().decode(Reply.self, from: data)
        let beside = try? JSONDecoder().decode(Replay.self, from: data)
        d.own = beside?.own
        print("\(f): \(classify(d.reply).1)")
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
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    app.run()
}
