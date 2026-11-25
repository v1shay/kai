import AppKit
import Metal

@objc(GPUIView)
final class ProbeEditor: NSTextView {}

// Keep lifecycle assertions deterministic without moving a window over the user.
// These assertions verify the actual Ghostty surface's visibility gate; physical
// WindowServer presentation is covered by the packaged Metal probe.
final class OcclusionProbeWindow: NSWindow {
    var probeOcclusion: NSWindow.OcclusionState = [.visible]
    override var occlusionState: NSWindow.OcclusionState { probeOcclusion }
}

@main
struct TypingResponderHarness {
    @MainActor static func main() {
        precondition(MTLCreateSystemDefaultDevice() != nil)
        NSApplication.shared.setActivationPolicy(.accessory)
        let window = OcclusionProbeWindow(contentRect: CGRect(x: -3000, y: -3000, width: 800, height: 480),
                              styleMask: [.titled], backing: .buffered, defer: false)
        let content = window.contentView!
        let editor = ProbeEditor(frame: content.bounds)
        content.addSubview(editor)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(editor)
        precondition(content !== editor)
        precondition(WorkspaceInputFocus.editor(in: window) === editor)
        let manager = UnifiedChatTerminal()
        let bounds = CGRect(x: 400, y: 0, width: 400, height: 480)
        manager.setVisibleChats(active: "gui", peer: "cli")
        manager.setChatBounds(bounds, for: "cli")
        let arrowProbe = #"""
        exec /usr/bin/python3 -c 'import os, termios, time, tty
        fd = 0
        previous = termios.tcgetattr(fd)
        tty.setraw(fd)
        os.write(1, b"KAI_KEYS_READY\r\n")
        value = os.read(fd, 64)
        termios.tcsetattr(fd, termios.TCSANOW, previous)
        print("KAI_ARROW_BYTES=" + value.hex(), flush=True)
        time.sleep(60)'
        """#
        manager.open(id: "cli", arguments: ["-c", arrowProbe], directory: "/tmp",
                     title: "CLI", window: window, environment: [], launchId: "fixture")
        let terminal = content.subviews.flatMap(\.subviews).compactMap { $0 as? WorkspaceGhosttyView }.first!
        precondition(terminal.isRenderingVisible)
        func awaitTerminal(_ marker: String) -> Bool {
            let timeout = Date().addingTimeInterval(4)
            while Date() < timeout {
                if terminal.probeContains(marker) { return true }
                RunLoop.main.run(until: Date().addingTimeInterval(0.025))
            }
            return false
        }
        precondition(awaitTerminal("KAI_KEYS_READY"), "The Ghostty PTY did not start")
        let up = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.function, .numericPad],
                                 timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                 context: nil, characters: "\u{F700}", charactersIgnoringModifiers: "\u{F700}",
                                 isARepeat: false, keyCode: 126)!
        precondition(window.makeFirstResponder(terminal))
        window.sendEvent(up)
        precondition(awaitTerminal("KAI_ARROW_BYTES="), "The PTY received no key")
        precondition(terminal.probeContains("KAI_ARROW_BYTES=1b5b41") ||
                     terminal.probeContains("KAI_ARROW_BYTES=1b4f41"), "The PTY received the wrong Up Arrow sequence")
        window.makeFirstResponder(editor)
        var visibilityChanges = 0
        terminal.onRenderingVisibilityChange = { visibilityChanges += 1 }
        terminal.setRenderingVisible(true)
        terminal.setRenderingVisible(true)
        precondition(visibilityChanges == 0, "Repeated visible updates must not refresh the Metal surface")
        window.probeOcclusion = []
        NotificationCenter.default.post(name: NSWindow.didChangeOcclusionStateNotification, object: window)
        precondition(!terminal.isRenderingVisible, "Covered windows must stop terminal rendering")
        window.probeOcclusion = [.visible]
        NotificationCenter.default.post(name: NSWindow.didChangeOcclusionStateNotification, object: window)
        precondition(terminal.isRenderingVisible, "Uncovering a selected terminal must resume rendering")
        manager.select(id: "cli", launchId: "fixture", selected: false)
        NotificationCenter.default.post(name: NSWindow.didChangeOcclusionStateNotification, object: window)
        precondition(!terminal.isRenderingVisible, "Uncovering must preserve logical tab visibility")
        manager.select(id: "cli", launchId: "fixture", selected: true)
        precondition(terminal.isRenderingVisible)
        terminal.onRenderingVisibilityChange = nil
        func event(_ type: NSEvent.EventType, x: CGFloat) -> NSEvent {
            NSEvent.mouseEvent(with: type, location: CGPoint(x: x, y: 200), modifierFlags: [],
                              timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                              context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        }
        for index in 0..<30 {
            terminal.mouseDown(with: event(.leftMouseDown, x: 600))
            precondition(window.firstResponder === terminal)
            WorkspaceInputFocus.reclaimForEditorClick(event(.leftMouseDown, x: 600), in: window)
            precondition(window.firstResponder === terminal, "A click on the terminal must keep its keyboard")
            if index.isMultiple(of: 3) {
                WorkspaceInputFocus.reclaimForEditorClick(event(.leftMouseDown, x: 150), in: window)
            } else if index.isMultiple(of: 2) {
                manager.select(id: "cli", launchId: "fixture", selected: false)
                manager.select(id: "cli", launchId: "fixture", selected: true)
            } else {
                manager.setVisibleChats(active: "cli", peer: "gui")
                manager.setVisibleChats(active: "gui", peer: "cli")
            }
            precondition(window.firstResponder === editor, "Editor lost keyboard focus during transition \(index)")
            let key = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
                                      timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                                      context: nil, characters: "a", charactersIgnoringModifiers: "a", isARepeat: false, keyCode: 0)!
            window.sendEvent(key)
        }
        precondition(editor.string == String(repeating: "a", count: 30))
        window.makeFirstResponder(nil)
        WorkspaceInputFocus.repairMissingEditorResponder(in: window)
        precondition(window.firstResponder === editor)
        manager.close(id: "cli", launchId: "fixture")
        RunLoop.main.run(until: Date().addingTimeInterval(0.25))
        print("PASS: Metal surface occlusion/resume, hidden-tab retention, redundant-refresh coalescing; 30 real AppKit keyboard handoffs, terminal hit testing, editor clicks, tab switches, and responder recovery")
    }
}
