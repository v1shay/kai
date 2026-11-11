import AppKit
import Metal

@objc(GPUIView)
final class ProbeEditor: NSTextView {}

@main
struct TypingResponderHarness {
    @MainActor static func main() {
        precondition(MTLCreateSystemDefaultDevice() != nil)
        NSApplication.shared.setActivationPolicy(.accessory)
        let window = NSWindow(contentRect: CGRect(x: -3000, y: -3000, width: 800, height: 480),
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
        manager.open(id: "cli", arguments: ["-c", "read line"], directory: "/tmp",
                     title: "CLI", window: window, environment: [], launchId: "fixture")
        let terminal = content.subviews.flatMap(\.subviews).compactMap { $0 as? WorkspaceGhosttyView }.first!
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
        print("PASS: 30 real AppKit keyboard handoffs, terminal hit testing, editor clicks, tab switches, and responder recovery")
    }
}
