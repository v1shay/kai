import AppKit
import SwiftUI

struct KaiRemoteTerminalView: View {
    let arguments: [String]

    var body: some View {
        #if canImport(libghostty)
        KaiRemoteTerminalSurface(arguments: arguments)
        #else
        Text(arguments.map(quoteShellArgument).joined(separator: " "))
            .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
        #endif
    }
}

private func quoteShellArgument(_ value: String) -> String {
    "'" + value.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
}

#if canImport(libghostty)
private struct KaiRemoteTerminalSurface: NSViewRepresentable {
    let arguments: [String]

    func makeNSView(context: Context) -> KaiRemoteTerminalContainer {
        KaiRemoteTerminalContainer(arguments: arguments)
    }
    func updateNSView(_ surface: KaiRemoteTerminalContainer, context: Context) {}
    static func dismantleNSView(_ surface: KaiRemoteTerminalContainer, coordinator: ()) {
        surface.stop()
    }
}

private final class KaiRemoteTerminalContainer: NSView {
    private let terminal = WorkspaceGhosttyView(frame: .zero)
    private let command: String
    private var started = false

    init(arguments: [String]) {
        command = arguments.map(quoteShellArgument).joined(separator: " ")
        super.init(frame: .zero)
        addSubview(terminal)
        terminal.autoresizingMask = [.width, .height]
    }

    required init?(coder: NSCoder) { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil, !started else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, window != nil, !started else { return }
            terminal.frame = bounds
            started = terminal.start(directory: FileManager.default.homeDirectoryForCurrentUser,
                                     command: command, waitAfterCommand: true)
            if !started {
                let fallback = NSTextField(wrappingLabelWithString: command)
                fallback.isSelectable = true
                fallback.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
                fallback.frame = bounds.insetBy(dx: 10, dy: 10)
                fallback.autoresizingMask = [.width, .height]
                addSubview(fallback)
            }
        }
    }

    func stop() {
        terminal.stop()
        started = false
    }
}
#endif
