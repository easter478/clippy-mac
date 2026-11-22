// clippy-helper: the Mac-side senses for the Clippy app (a sidecar inside Clippy.app).
//
//   clippy-helper watch          JSON lines on stdout, one per event, until stdin closes:
//       {"t":"key","combo":"ctrl+c","app":"Notes"}   a Windows-style shortcut (only Ctrl/Alt combos with a Mac
//                                                    equivalent; letters typed on their own are never looked at)
//       {"t":"idle","secs":12}                      seconds since the last mouse/key input, every 15 s
//       {"t":"keys","ok":false}                     Input Monitoring is not allowed yet (tips can't work)
//   clippy-helper health             {"cpu":…,"mem":…,"disk":…,"diskFreeGB":…,"uptimeH":…}
//   clippy-helper keys-request       asks macOS for Input Monitoring (shows the system prompt once)
import AppKit
import CoreGraphics
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)

func emit(_ obj: [String: Any]) {
    guard let d = try? JSONSerialization.data(withJSONObject: obj), let s = String(data: d, encoding: .utf8) else { return }
    print(s)
    fflush(stdout)
}

// ---------- health ----------
func health() {
    var load = [Double](repeating: 0, count: 3)
    getloadavg(&load, 3)
    let cores = Double(ProcessInfo.processInfo.activeProcessorCount)
    var stats = vm_statistics64()
    var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64_data_t>.size / MemoryLayout<integer_t>.size)
    let kr = withUnsafeMutablePointer(to: &stats) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count) }
    }
    var page: vm_size_t = 0
    host_page_size(mach_host_self(), &page)
    let total = Double(ProcessInfo.processInfo.physicalMemory)
    var mem = 0.0
    if kr == KERN_SUCCESS {
        let used = Double(UInt64(stats.active_count) + UInt64(stats.wire_count) + UInt64(stats.compressor_page_count)) * Double(page)
        mem = min(100, used / total * 100)
    }
    var disk = 0.0, free = 0.0
    if let v = try? URL(fileURLWithPath: "/").resourceValues(forKeys: [.volumeTotalCapacityKey, .volumeAvailableCapacityForImportantUsageKey]),
       let t = v.volumeTotalCapacity, let a = v.volumeAvailableCapacityForImportantUsage, t > 0 {
        free = Double(a) / 1_000_000_000
        disk = (1 - Double(a) / Double(t)) * 100
    }
    emit(["cpu": (min(100, load[0] / cores * 100) * 10).rounded() / 10, "mem": (mem * 10).rounded() / 10,
          "disk": (disk * 10).rounded() / 10, "diskFreeGB": (free * 10).rounded() / 10,
          "memGB": (total / 1_073_741_824).rounded(), "uptimeH": (ProcessInfo.processInfo.systemUptime / 360).rounded() / 10])
}

// ---------- watch ----------
// Ctrl/Alt combos Windows users press out of habit → the Mac shortcut. Key codes are physical (ANSI) positions.
let CTRL_KEYS: [Int64: String] = [8: "c", 9: "v", 7: "x", 6: "z", 0: "a", 1: "s", 3: "f", 13: "w", 17: "t", 45: "n",
                                  35: "p", 15: "r", 16: "y", 31: "o"]
// apps where Ctrl/Alt keys are normal (terminals, editors with terminals, remote Windows desktops, VMs)
let SKIP_APPS: Set<String> = ["com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "com.mitchellh.ghostty",
    "net.kovidgoyal.kitty", "org.alacritty", "com.github.wez.wezterm", "com.microsoft.VSCode", "com.todesktop.230313mzl4w4u92",
    "com.microsoft.rdc.macos", "com.microsoft.rdc.mac", "com.parallels.desktop.console", "com.vmware.fusion", "com.utmapp.UTM",
    "com.apple.ScreenSharing", "com.anthropic.claudefordesktop", "com.jetbrains.intellij", "com.sublimetext.4", "org.vim.MacVim"]

var lastIdleEmit = Date.distantPast

func startKeyTap() -> Bool {
    guard CGPreflightListenEventAccess() else { return false }
    let mask = CGEventMask(1 << CGEventType.keyDown.rawValue)
    let cb: CGEventTapCallBack = { _, type, event, _ in
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput { return Unmanaged.passUnretained(event) }
        let f = event.flags
        let code = event.getIntegerValueField(.keyboardEventKeycode)
        let cmd = f.contains(.maskCommand), ctrl = f.contains(.maskControl), alt = f.contains(.maskAlternate)
        var combo: String? = nil
        if ctrl && !cmd && !alt, let k = CTRL_KEYS[code] { combo = "ctrl+" + k }
        else if alt && !cmd && !ctrl && code == 48 { combo = "alt+tab" }
        else if alt && !cmd && !ctrl && code == 118 { combo = "alt+f4" }
        if let combo = combo, event.getIntegerValueField(.keyboardEventAutorepeat) == 0 {
            let app = NSWorkspace.shared.frontmostApplication
            if !SKIP_APPS.contains(app?.bundleIdentifier ?? "") {
                emit(["t": "key", "combo": combo, "app": app?.localizedName ?? ""])
            }
        }
        return Unmanaged.passUnretained(event)
    }
    guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly,
                                      eventsOfInterest: mask, callback: cb, userInfo: nil) else { return false }
    let src = CFMachPortCreateRunLoopSource(nil, tap, 0)
    CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
    CGEvent.tapEnable(tap: tap, enable: true)
    return true
}

func watch() {
    // stop when Clippy (the parent) goes away
    FileHandle.standardInput.readabilityHandler = { h in if h.availableData.isEmpty { exit(0) } }
    let keysOK = startKeyTap()
    emit(["t": "keys", "ok": keysOK])
    var keysOn = keysOK

    Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { _ in
        let any = CGEventType(rawValue: ~0)!
        let secs = CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: any)
        emit(["t": "idle", "secs": Int(secs)])
        if !keysOn && CGPreflightListenEventAccess() {   // allowed while running: start the tips now
            keysOn = startKeyTap()
            if keysOn { emit(["t": "keys", "ok": true]) }
        }
    }
    RunLoop.main.run()
}

let args = CommandLine.arguments
switch args.count > 1 ? args[1] : "" {
case "watch":
    watch()
case "health":
    health()
case "keys-request":
    print(CGRequestListenEventAccess() ? "ok" : "asked")
default:
    print("usage: clippy-helper watch | health | keys-request")
    exit(2)
}
