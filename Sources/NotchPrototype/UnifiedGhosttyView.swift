#if canImport(libghostty)
import AppKit
import libghostty
import Metal

@MainActor
private final class WorkspaceGhosttyRuntime {
    static let shared = WorkspaceGhosttyRuntime()
    static let hasMetalDevice = MTLCreateSystemDefaultDevice() != nil
    private(set) var app: ghostty_app_t?
    private var tickQueued = false

    private init() {
        guard ghostty_init(0, nil) == GHOSTTY_SUCCESS, let config = ghostty_config_new() else { return }
        defer { ghostty_config_free(config) }
        if let url = Bundle.main.url(forResource: "workspace-ghostty", withExtension: "conf") {
            url.path.withCString { ghostty_config_load_file(config, $0) }
        }
        ghostty_config_finalize(config)
        var callbacks = ghostty_runtime_config_s()
        callbacks.userdata = Unmanaged.passUnretained(self).toOpaque()
        callbacks.wakeup_cb = { _ in
            DispatchQueue.main.async { WorkspaceGhosttyRuntime.shared.queueTick() }
        }
        callbacks.action_cb = { _, target, action in
            guard target.tag == GHOSTTY_TARGET_SURFACE else { return false }
            let surface = target.target.surface
            if action.tag == GHOSTTY_ACTION_RENDER {
                ghostty_surface_draw(surface)
                return true
            }
            if action.tag == GHOSTTY_ACTION_SHOW_CHILD_EXITED,
               let pointer = ghostty_surface_userdata(surface) {
                let view = Unmanaged<WorkspaceGhosttyView>.fromOpaque(pointer).takeUnretainedValue()
                DispatchQueue.main.async { [weak view] in view?.onProcessExit?() }
                return true
            }
            return false
        }
        callbacks.read_clipboard_cb = { _, _, _, _, _, _ in GHOSTTY_CLIPBOARD_READ_UNSUPPORTED }
        callbacks.write_clipboard_cb = { _, _, _, _, _ in }
        callbacks.confirm_read_clipboard_cb = { _, _, _, _ in }
        callbacks.close_surface_cb = { _, _ in }
        app = ghostty_app_new(&callbacks, config)
        if let app { ghostty_app_set_focus(app, true) }
    }

    private func queueTick() {
        guard !tickQueued else { return }
        tickQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self, let app = self.app else { return }
            self.tickQueued = false
            ghostty_app_tick(app)
        }
    }
}

final class WorkspaceGhosttyView: NSView, NSTextInputClient {
    private var surface: ghostty_surface_t?
    private var preedit = NSAttributedString()
    private var preeditSelection = NSRange(location: NSNotFound, length: 0)
    private var keyText: String?
    var onFocus: (() -> Void)?
    var onProcessExit: (() -> Void)?
    var onToggleChat: (() -> Void)?

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    func start(directory: URL, command: String?, environment overrides: [[String]] = [], waitAfterCommand: Bool = true) -> Bool {
        guard WorkspaceGhosttyRuntime.hasMetalDevice, let app = WorkspaceGhosttyRuntime.shared.app, let window else { return false }
        wantsLayer = true
        let pointer = Unmanaged.passUnretained(self).toOpaque()
        var config = ghostty_surface_config_new()
        config.platform_tag = GHOSTTY_PLATFORM_MACOS
        config.platform = ghostty_platform_u(macos: ghostty_platform_macos_s(nsview: pointer))
        config.userdata = pointer
        config.scale_factor = window.backingScaleFactor
        config.font_size = 13
        config.wait_after_command = waitAfterCommand
        config.context = GHOSTTY_SURFACE_CONTEXT_TAB
        let searchPath = ["/opt/homebrew/bin", "/usr/local/bin", NSHomeDirectory() + "/.local/bin",
                          ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin"].joined(separator: ":")
        let terminfo = Bundle.main.resourceURL?.appendingPathComponent("terminfo").path ?? "/usr/share/terminfo"
        var environment = ["PATH": searchPath, "TERMINFO_DIRS": terminfo + ":",
                           "TERM_PROGRAM": "ghostty", "TERM_PROGRAM_VERSION": "1.4.0"]
        for pair in overrides where pair.count == 2 {
            let name = pair[0], value = pair[1]
            guard !name.isEmpty, !name.contains("="), !name.utf8.contains(0), !value.utf8.contains(0) else { continue }
            environment[name] = value
        }
        let pairs = environment.sorted { $0.key < $1.key }
        let strings = pairs.flatMap { [strdup($0.key), strdup($0.value)] }
        defer { strings.forEach { free($0) } }
        guard strings.allSatisfy({ $0 != nil }) else { return false }
        var variables = pairs.indices.map { index in
            ghostty_env_var_s(key: UnsafePointer(strings[2 * index]!), value: UnsafePointer(strings[2 * index + 1]!))
        }
        surface = directory.path.withCString { path in
            config.working_directory = path
            return variables.withUnsafeMutableBufferPointer { buffer in
                config.env_vars = buffer.baseAddress
                config.env_var_count = buffer.count
                guard let command else { return ghostty_surface_new(app, &config) }
                return command.withCString {
                    config.command = $0
                    return ghostty_surface_new(app, &config)
                }
            }
        }
        guard let surface else { return false }
        updateSize()
        ghostty_surface_refresh(surface)
        ghostty_surface_draw(surface)
        return true
    }

    func setRenderingVisible(_ visible: Bool) {
        if let surface {
            ghostty_surface_set_occlusion(surface, visible)
            if visible { ghostty_surface_refresh(surface) }
        }
    }

    func stop() {
        guard let surface else { return }
        self.surface = nil
        ghostty_surface_free(surface)
    }

    deinit {
        if let surface { ghostty_surface_free(surface) }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let surface { ghostty_surface_set_occlusion(surface, window != nil) }
        updateSize()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        updateSize()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        updateSize()
    }

    private func updateSize() {
        guard let surface, let window, bounds.width > 0, bounds.height > 0 else { return }
        let scale = window.backingScaleFactor
        ghostty_surface_set_content_scale(surface, scale, scale)
        ghostty_surface_set_size(surface, UInt32(bounds.width * scale), UInt32(bounds.height * scale))
    }

    override func becomeFirstResponder() -> Bool {
        if let surface { ghostty_surface_set_focus(surface, true) }
        onFocus?()
        return true
    }

    override func resignFirstResponder() -> Bool {
        inputContext?.discardMarkedText()
        unmarkText()
        if let surface { ghostty_surface_set_focus(surface, false) }
        return true
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard window?.firstResponder === self else { return false }
        let flags = event.modifierFlags.intersection([.command, .shift, .control, .option])
        if flags == [.command, .shift], event.charactersIgnoringModifiers?.lowercased() == "j",
           let onToggleChat {
            onToggleChat()
            return true
        }
        if flags == .command {
            switch event.charactersIgnoringModifiers?.lowercased() {
            case "c": copySelection(); return true
            case "v": paste(); return true
            case "a": selectAll(nil); return true
            case "q": NSApp.terminate(nil); return true
            case "m": window?.miniaturize(nil); return true
            default:
                switch event.keyCode {
                case 123: input("\u{01}"); return true
                case 124: input("\u{05}"); return true
                case 51: input("\u{15}"); return true
                case 36, 76: input("\r"); return true
                default: break
                }
            }
        }
        return super.performKeyEquivalent(with: event)
    }

    override func keyDown(with event: NSEvent) {
        guard let surface else { return }
        let translation = ghostty_surface_key_translation_mods(surface, mods(event.modifierFlags))
        let translated = translationEvent(event, using: translation)
        let wasComposing = hasMarkedText()
        keyText = ""
        defer { keyText = nil }

        // AppKit's text input context handles dead keys, keyboard layouts, and IMEs.
        // Keep the physical key too, so navigation and shell controls remain key events.
        interpretKeyEvents([translated])
        let committed = keyText ?? ""
        let action = event.isARepeat ? GHOSTTY_ACTION_REPEAT : GHOSTTY_ACTION_PRESS
        if wasComposing {
            if !committed.isEmpty && !committed.unicodeScalars.allSatisfy({ $0.value < 32 || $0.value == 127 }) {
                sendCommittedText(committed)
            }
            return
        }
        if hasMarkedText() { return }
        let text = committed.isEmpty ? terminalCharacters(translated) : committed
        sendKey(event, action: action, translatedFlags: translated.modifierFlags, text: text)
    }

    override func keyUp(with event: NSEvent) {
        sendKey(event, action: GHOSTTY_ACTION_RELEASE, translatedFlags: event.modifierFlags)
    }

    private func translationEvent(_ event: NSEvent, using mods: ghostty_input_mods_e) -> NSEvent {
        var flags = event.modifierFlags
        let modifiers: [(NSEvent.ModifierFlags, ghostty_input_mods_e)] = [
            (.shift, GHOSTTY_MODS_SHIFT), (.control, GHOSTTY_MODS_CTRL),
            (.option, GHOSTTY_MODS_ALT), (.command, GHOSTTY_MODS_SUPER),
        ]
        for (flag, modifier) in modifiers {
            if mods.rawValue & modifier.rawValue != 0 { flags.insert(flag) }
            else { flags.remove(flag) }
        }
        guard flags != event.modifierFlags else { return event }
        return NSEvent.keyEvent(with: event.type, location: event.locationInWindow, modifierFlags: flags,
                                timestamp: event.timestamp, windowNumber: event.windowNumber, context: nil,
                                characters: event.characters(byApplyingModifiers: flags) ?? "",
                                charactersIgnoringModifiers: event.charactersIgnoringModifiers ?? "",
                                isARepeat: event.isARepeat, keyCode: event.keyCode) ?? event
    }

    private func terminalCharacters(_ event: NSEvent) -> String? {
        guard let text = event.characters, let first = text.unicodeScalars.first else { return nil }
        if text.unicodeScalars.count == 1 {
            if first.value < 32 { return event.characters(byApplyingModifiers: event.modifierFlags.subtracting(.control)) }
            if (0xF700...0xF8FF).contains(first.value) { return nil }
        }
        return text
    }

    private func sendKey(_ event: NSEvent, action: ghostty_input_action_e,
                         translatedFlags: NSEvent.ModifierFlags, text: String? = nil) {
        guard let surface else { return }
        var key = ghostty_input_key_s(action: action, mods: mods(event.modifierFlags),
                                     consumed_mods: mods(translatedFlags.subtracting([.control, .command])),
                                     keycode: UInt32(event.keyCode), text: nil,
                                     unshifted_codepoint: event.characters(byApplyingModifiers: [])?.unicodeScalars.first?.value ?? 0,
                                     composing: hasMarkedText())
        if let text, !text.isEmpty {
            text.withCString { key.text = $0; _ = ghostty_surface_key(surface, key) }
        } else {
            _ = ghostty_surface_key(surface, key)
        }
    }

    private func sendCommittedText(_ text: String) {
        guard let surface else { return }
        var key = ghostty_input_key_s(action: GHOSTTY_ACTION_PRESS, mods: GHOSTTY_MODS_NONE,
                                     consumed_mods: GHOSTTY_MODS_NONE, keycode: UInt32.max, text: nil,
                                     unshifted_codepoint: 0, composing: false)
        text.withCString { key.text = $0; _ = ghostty_surface_key(surface, key) }
    }

    func hasMarkedText() -> Bool { preedit.length > 0 }

    func markedRange() -> NSRange {
        hasMarkedText() ? NSRange(location: 0, length: preedit.length) : NSRange(location: NSNotFound, length: 0)
    }

    func selectedRange() -> NSRange {
        hasMarkedText() ? preeditSelection : NSRange(location: NSNotFound, length: 0)
    }

    func setMarkedText(_ value: Any, selectedRange: NSRange, replacementRange: NSRange) {
        if let attributed = value as? NSAttributedString { preedit = attributed }
        else if let text = value as? String { preedit = NSAttributedString(string: text) }
        else { return }
        preeditSelection = selectedRange
        guard let surface else { return }
        let text = preedit.string
        text.withCString { ghostty_surface_preedit(surface, $0, UInt(text.utf8.count)) }
    }

    func unmarkText() {
        guard hasMarkedText() else { return }
        preedit = NSAttributedString()
        preeditSelection = NSRange(location: NSNotFound, length: 0)
        if let surface { ghostty_surface_preedit(surface, nil, 0) }
    }

    func insertText(_ value: Any, replacementRange: NSRange) {
        guard let text = (value as? NSAttributedString)?.string ?? value as? String, !text.isEmpty else { return }
        unmarkText()
        if keyText != nil { keyText?.append(text) }
        else { sendCommittedText(text) }
    }

    override func doCommand(by selector: Selector) {
        // Key events without committed text are encoded by Ghostty.
    }

    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        guard hasMarkedText(), range.location != NSNotFound, range.location <= preedit.length,
              range.length <= preedit.length - range.location else { return nil }
        actualRange?.pointee = range
        return preedit.attributedSubstring(from: range)
    }

    func characterIndex(for point: NSPoint) -> Int { NSNotFound }

    func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        guard let surface, let window else { return .zero }
        var x = 0.0, y = 0.0, width = 0.0, height = 0.0
        ghostty_surface_ime_point(surface, &x, &y, &width, &height)
        actualRange?.pointee = range
        return window.convertToScreen(convert(NSRect(x: x, y: bounds.height - y, width: width, height: height), to: nil))
    }

    private func mods(_ flags: NSEvent.ModifierFlags) -> ghostty_input_mods_e {
        var mask = GHOSTTY_MODS_NONE.rawValue
        if flags.contains(.shift) { mask |= GHOSTTY_MODS_SHIFT.rawValue }
        if flags.contains(.capsLock) { mask |= GHOSTTY_MODS_CAPS.rawValue }
        if flags.contains(.control) { mask |= GHOSTTY_MODS_CTRL.rawValue }
        if flags.contains(.option) { mask |= GHOSTTY_MODS_ALT.rawValue }
        if flags.contains(.command) { mask |= GHOSTTY_MODS_SUPER.rawValue }
        return ghostty_input_mods_e(rawValue: mask)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        mouse(event, state: GHOSTTY_MOUSE_PRESS, button: GHOSTTY_MOUSE_LEFT)
    }

    override func mouseUp(with event: NSEvent) { mouse(event, state: GHOSTTY_MOUSE_RELEASE, button: GHOSTTY_MOUSE_LEFT) }
    override func rightMouseDown(with event: NSEvent) { mouse(event, state: GHOSTTY_MOUSE_PRESS, button: GHOSTTY_MOUSE_RIGHT) }
    override func rightMouseUp(with event: NSEvent) { mouse(event, state: GHOSTTY_MOUSE_RELEASE, button: GHOSTTY_MOUSE_RIGHT) }
    override func mouseDragged(with event: NSEvent) { mouseMoved(with: event) }

    override func mouseMoved(with event: NSEvent) {
        guard let surface else { return }
        let point = convert(event.locationInWindow, from: nil)
        ghostty_surface_mouse_pos(surface, point.x, bounds.height - point.y, mods(event.modifierFlags))
    }

    private func mouse(_ event: NSEvent, state: ghostty_input_mouse_state_e, button: ghostty_input_mouse_button_e) {
        guard let surface else { return }
        mouseMoved(with: event)
        _ = ghostty_surface_mouse_button(surface, state, button, mods(event.modifierFlags))
    }

    override func scrollWheel(with event: NSEvent) {
        guard let surface else { return }
        let precise = event.hasPreciseScrollingDeltas
        let scale = precise ? 2.0 : 1.0
        ghostty_surface_mouse_scroll(surface, event.scrollingDeltaX * scale,
                                     event.scrollingDeltaY * scale, precise ? 1 : 0)
    }

    func paste() {
        guard let text = NSPasteboard.general.string(forType: .string) else { return }
        input(text)
    }

    func input(_ text: String) {
        guard let surface else { return }
        text.withCString { ghostty_surface_text(surface, $0, UInt(text.utf8.count)) }
    }

    @objc func copy(_ sender: Any?) { copySelection() }
    @objc func paste(_ sender: Any?) { paste() }
    override func selectAll(_ sender: Any?) {
        guard let surface else { return }
        let action = "select_all"
        action.withCString { _ = ghostty_surface_binding_action(surface, $0, UInt(action.utf8.count)) }
    }

    func copySelection() {
        guard let surface else { return }
        var text = ghostty_text_s()
        guard ghostty_surface_read_selection(surface, &text) else { return }
        defer { ghostty_surface_free_text(surface, &text) }
        guard let bytes = text.text else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(String(decoding: Data(bytes: bytes, count: Int(text.text_len)), as: UTF8.self), forType: .string)
    }
}

#endif

#if canImport(libghostty)
/// The workspace editor is a child of AppKit's content container. The
/// container itself does not receive GPUI keyboard and IME events.
@MainActor
enum WorkspaceInputFocus {
    static func editor(in window: NSWindow) -> NSView? {
        guard let editorClass = NSClassFromString("GPUIView") else { return nil }
        return window.contentView?.subviews.first { $0.isKind(of: editorClass) }
    }

    static func reclaimForEditorClick(_ event: NSEvent, in window: NSWindow) {
        guard event.window === window, let content = window.contentView,
              let editor = editor(in: window), window.firstResponder !== editor,
              let parent = content.superview else { return }
        let location = parent.convert(event.locationInWindow, from: nil)
        guard let hit = content.hitTest(location), hit === editor || hit.isDescendant(of: editor) else { return }
        window.makeFirstResponder(editor)
    }

    static func repairMissingEditorResponder(in window: NSWindow) {
        let current = window.firstResponder
        guard current == nil || current === window || current === window.contentView,
              let editor = editor(in: window) else { return }
        window.makeFirstResponder(editor)
    }
}

/// A dock panel owns a direct Ghostty PTY. GPUI keeps the dock header and
/// layout controls, and the native surface draws only inside the viewport.
@MainActor
final class DockedGhosttyTerminals {
    @MainActor
    private final class Surface {
        let clipView = NSView()
        let terminal = WorkspaceGhosttyView()
        var requestedFocus = false
    }

    private var surfaces: [String: Surface] = [:]
    private var closing = Set<String>()

    func update(id: String, directory: String?, rect: CGRect, clip: CGRect, focused: Bool, window: NSWindow) {
        guard !closing.contains(id), let parent = window.contentView else { return }
        let surface: Surface
        if let existing = surfaces[id] {
            surface = existing
        } else {
            guard directory != nil else { return }
            surface = Surface()
            surface.clipView.wantsLayer = true
            surface.clipView.layer?.masksToBounds = true
            surface.clipView.addSubview(surface.terminal)
            parent.addSubview(surface.clipView)
            surfaces[id] = surface
        }
        let frame = CGRect(x: clip.minX, y: parent.isFlipped ? clip.minY : parent.bounds.height - clip.maxY,
                           width: clip.width, height: clip.height)
        let terminalFrame = CGRect(x: rect.minX - clip.minX, y: clip.maxY - rect.maxY,
                                   width: rect.width, height: rect.height)
        if surface.clipView.frame != frame { surface.clipView.frame = frame }
        if surface.terminal.frame != terminalFrame { surface.terminal.frame = terminalFrame }
        if let directory {
            surface.terminal.onProcessExit = { [weak self] in self?.close(id: id) }
            if surface.terminal.start(directory: URL(fileURLWithPath: directory, isDirectory: true),
                                      command: nil, waitAfterCommand: false) {
                EmbeddedCodeRuntime.shared.sendAction("dock_terminal_ready", ["id": id])
            } else {
                close(id: id, error: "Could not start Ghostty Metal. Check the terminal’s working directory.")
                return
            }
        }
        if surface.clipView.isHidden {
            surface.clipView.isHidden = false
            surface.terminal.setRenderingVisible(true)
        }
        if focused, !surface.requestedFocus, window.isKeyWindow {
            window.makeFirstResponder(surface.terminal)
        }
        surface.requestedFocus = focused
    }

    func hide(id: String) {
        guard let surface = surfaces[id], !surface.clipView.isHidden else { return }
        if let window = surface.terminal.window, window.firstResponder === surface.terminal,
           let editor = WorkspaceInputFocus.editor(in: window) {
            window.makeFirstResponder(editor)
        }
        surface.clipView.isHidden = true
        surface.terminal.setRenderingVisible(false)
        surface.requestedFocus = false
    }

    func close(id: String, error: String? = nil) {
        guard let surface = surfaces[id] else { return }
        hide(id: id)
        surfaces.removeValue(forKey: id)
        surface.terminal.onProcessExit = nil
        surface.clipView.removeFromSuperview()
        closing.insert(id)
        // Let the dock update its frame before Ghostty joins its threads.
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(120)) { [self] in
            surface.terminal.stop()
            closing.remove(id)
            var fields: [String: Any] = ["id": id]
            if let error { fields["error"] = error }
            EmbeddedCodeRuntime.shared.sendAction("dock_terminal_closed", fields)
        }
    }

    func sendCommand(_ command: String, from responder: WorkspaceGhosttyView) -> Bool {
        guard let (id, _) = surfaces.first(where: {
            !$0.value.clipView.isHidden && $0.value.terminal === responder
        }) else { return false }
        return EmbeddedCodeRuntime.shared.sendAction("dock_terminal_command", ["id": id, "command": command])
    }
}

/// Owns the surfaces for open chat tabs and gives the foreground tab the workspace pane.
@MainActor
final class UnifiedChatTerminal {
    private var surfaces: [String: ChatTerminalSurface] = [:]
    private var retired: [(surface: ChatTerminalSurface, acknowledge: Bool)] = []
    private var activeChat: String?
    private var peerChats: Set<String> = []
    private var chatBounds: [String: CGRect] = [:]
    private var pendingFocus: String?

    func setVisibleChats(active id: String?, peer: String?) {
        setVisibleChats(active: id, peers: peer.map { [$0] } ?? [])
    }

    func setVisibleChats(active id: String?, peers: [String]) {
        let focusChanged = activeChat != id
        activeChat = id
        if focusChanged { pendingFocus = id }
        peerChats = Set(peers)
        chatBounds = chatBounds.filter { $0.key == id || peerChats.contains($0.key) }
        if focusChanged {
            for (owner, surface) in surfaces where owner != id {
                surface.restoreEditorFocus()
            }
        }
        for (owner, surface) in surfaces {
            update(surface, owner: owner, focus: focusChanged && owner == id)
        }
    }

    func setChatBounds(_ bounds: CGRect, for id: String) {
        guard id == activeChat || peerChats.contains(id) else { return }
        chatBounds[id] = bounds
        if let surface = surfaces[id] { update(surface, owner: id, focus: false) }
    }

    private func update(_ surface: ChatTerminalSurface, owner: String, focus: Bool) {
        let bounds = chatBounds[owner] ?? .zero
        let visible = (owner == activeChat || peerChats.contains(owner)) && bounds.width > 0 && bounds.height > 44
        let focus = visible && (focus || pendingFocus == owner)
        if focus { pendingFocus = nil }
        surface.setActive(visible, bounds: bounds, focus: focus)
    }

    func sendCommand(_ command: String, from responder: WorkspaceGhosttyView) -> Bool {
        guard let surface = surfaces.values.first(where: { $0.ownsResponder(responder) }) else { return false }
        return surface.sendCommand(command)
    }

    func open(id: String, arguments: [String], directory: String, title: String, window: NSWindow,
              environment: [[String]], launchId: String, agent: String = "Agent") {
        if let existing = surfaces[id] {
            guard existing.launchId != launchId else { return }
            close(id: id, launchId: existing.launchId)
        }
        let forceFallback = CommandLine.arguments.contains("--unified-smoke-test")
            && CommandLine.arguments.contains("--unified-chat-split-probe")
        guard !forceFallback,
              let surface = ChatTerminalSurface(id: id, launchId: launchId, arguments: arguments, directory: directory,
                                                title: title, agent: agent, environment: environment,
                                                bounds: chatBounds[id] ?? CGRect(x: 0, y: 0, width: 640, height: 400), window: window) else {
            EmbeddedCodeRuntime.shared.sendAction("chat_terminal_fallback", [
                "id": id, "launchId": launchId, "requestId": UUID().uuidString,
            ])
            return
        }
        surfaces[id] = surface
        surface.onExit = { [weak self, weak surface] in
            guard let self, let surface, self.surfaces[surface.id] === surface else { return }
            self.close(id: surface.id, launchId: surface.launchId)
            EmbeddedCodeRuntime.shared.sendAction("chat_terminal_closed", [
                "id": surface.id, "launchId": surface.launchId, "requestId": UUID().uuidString,
            ])
        }
        EmbeddedCodeRuntime.shared.sendAction("chat_terminal_metal_visibility", [
            "id": id, "launchId": launchId, "visible": true, "requestId": UUID().uuidString,
        ])
        update(surface, owner: id, focus: id == activeChat)
    }

    func select(id: String, launchId: String, selected: Bool) {
        guard let surface = surfaces[id], surface.launchId == launchId else { return }
        surface.setSelected(selected, focus: selected && id == activeChat)
    }

    func adopt(id: String, newId: String, launchId: String) {
        guard id != newId, surfaces[newId] == nil,
              let surface = surfaces[id], surface.launchId == launchId else { return }
        surfaces.removeValue(forKey: id)
        surface.id = newId
        surfaces[newId] = surface
        update(surface, owner: newId, focus: false)
    }

    func close(id: String, launchId: String, acknowledge: Bool = false) {
        guard let surface = surfaces[id], surface.launchId == launchId else {
            if acknowledge, let index = retired.firstIndex(where: { $0.surface.launchId == launchId }) {
                retired[index].acknowledge = true
            }
            return
        }
        surfaces.removeValue(forKey: id)
        surface.detach()
        retired.append((surface, acknowledge))
        if retired.count == 1 { scheduleDisposal() }
    }

    private func scheduleDisposal() {
        // Ghostty joins its renderer and I/O threads when freeing a surface.
        // Keep the AppKit teardown on main and give tab navigation a frame first.
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(120)) { [self] in
            let entry = retired.removeFirst()
            entry.surface.dispose()
            if entry.acknowledge {
                EmbeddedCodeRuntime.shared.sendAction("chat_terminal_closed", [
                    "id": entry.surface.id, "launchId": entry.surface.launchId,
                ])
            }
            if !retired.isEmpty { scheduleDisposal() }
        }
    }
}

@MainActor
private final class ChatTerminalSurface: NSObject {
    private weak var window: NSWindow?
    private weak var previousResponder: NSResponder?
    var id: String
    let launchId: String
    private let container: NSView
    private let terminal: WorkspaceGhosttyView
    private var active = false
    private var selected = true
    var onExit: (() -> Void)?

    init?(id: String, launchId: String, arguments: [String], directory: String, title: String, agent: String,
          environment: [[String]], bounds: CGRect, window: NSWindow) {
        guard let content = window.contentView else { return nil }
        self.id = id
        self.launchId = launchId
        self.window = window
        container = NSView()
        terminal = WorkspaceGhosttyView()
        super.init()
        setBounds(bounds, in: content)
        container.wantsLayer = true
        container.layer?.masksToBounds = true
        let height: CGFloat = 44
        terminal.frame = NSRect(x: 0, y: 0, width: container.bounds.width,
                                height: max(0, container.bounds.height - height))
        terminal.autoresizingMask = [.width, .height]
        container.addSubview(terminal)
        let label = NSTextField(labelWithString: "\(title)   ·   \(agent) CLI")
        label.textColor = .white
        label.lineBreakMode = .byTruncatingTail
        label.frame = NSRect(x: 84, y: container.bounds.height - 32, width: max(0, container.bounds.width - 320), height: 20)
        label.autoresizingMask = [.width, .minYMargin]
        container.addSubview(label)
        let button = NSButton(title: "Back to Chat  ⇧⌘J", target: self, action: #selector(toggle))
        button.frame = NSRect(x: container.bounds.width - 194, y: container.bounds.height - 38, width: 150, height: 30)
        button.autoresizingMask = [.minXMargin, .minYMargin]
        container.addSubview(button)
        let close = NSButton(title: "×", target: self, action: #selector(closeTab))
        close.isBordered = false
        close.font = .systemFont(ofSize: 20)
        close.toolTip = "Close tab (⌘W)"
        close.frame = NSRect(x: container.bounds.width - 40, y: container.bounds.height - 38, width: 28, height: 30)
        close.autoresizingMask = [.minXMargin, .minYMargin]
        container.addSubview(close)

        container.isHidden = true
        content.addSubview(container)
        terminal.onFocus = { [weak self] in
            guard let self, self.previousResponder == nil else { return }
            if let window = self.window { self.previousResponder = WorkspaceInputFocus.editor(in: window) }
        }
        terminal.onProcessExit = { [weak self] in self?.onExit?() }
        terminal.onToggleChat = { [weak self] in self?.toggle() }
        let command = (["/bin/sh"] + arguments).map {
            "'" + $0.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
        }.joined(separator: " ")
        guard terminal.start(directory: URL(fileURLWithPath: directory, isDirectory: true), command: command,
                             environment: environment) else {
            container.removeFromSuperview()
            return nil
        }
        terminal.setRenderingVisible(false)
    }

    func setActive(_ active: Bool, bounds: CGRect, focus: Bool) {
        self.active = active
        if active { setBounds(bounds) }
        updateVisibility(focus: focus)
    }

    func setBounds(_ bounds: CGRect) {
        if let parent = container.superview { setBounds(bounds, in: parent) }
    }

    private func setBounds(_ bounds: CGRect, in content: NSView) {
        let y = content.isFlipped ? bounds.minY : content.bounds.height - bounds.maxY
        container.frame = CGRect(x: bounds.minX, y: y, width: bounds.width, height: bounds.height)
    }

    func ownsResponder(_ responder: WorkspaceGhosttyView) -> Bool {
        !container.isHidden && terminal === responder
    }

    @discardableResult
    func sendCommand(_ command: String) -> Bool {
        guard !container.isHidden else { return false }
        return EmbeddedCodeRuntime.shared.sendAction("chat_terminal_command", [
            "id": id, "launchId": launchId, "command": command,
        ])
    }

    @objc private func closeTab() { sendCommand("close_tab") }

    @objc func toggle() { sendCommand("terminalize") }

    func setSelected(_ selected: Bool, focus: Bool) {
        self.selected = selected
        updateVisibility(focus: focus)
    }

    private func updateVisibility(focus: Bool) {
        let visible = active && selected
        let changed = visible == container.isHidden
        // AppKit can clear the first responder as soon as its view is hidden.
        // Hand the keyboard back while we still know this terminal owns it.
        if !visible { restoreEditorFocus() }
        if changed {
            container.isHidden = !visible
            terminal.setRenderingVisible(visible)
        }
        guard let window else { return }
        if !visible {
            previousResponder = nil
            return
        }
        if changed, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion, let layer = container.layer {
            let fade = CABasicAnimation(keyPath: "opacity")
            fade.fromValue = 0.0
            fade.toValue = 1.0
            fade.duration = KaiMotion.paneDuration
            fade.timingFunction = KaiMotion.interactionCurve
            layer.addKai(fade, forKey: "chat-terminal-reveal")
        }
        if focus, window.isKeyWindow, window.firstResponder !== terminal {
            if !(window.firstResponder is WorkspaceGhosttyView) { previousResponder = window.firstResponder }
            window.makeFirstResponder(terminal)
        }
    }

    func restoreEditorFocus() {
        guard let window, window.firstResponder === terminal else { return }
        if let editor = WorkspaceInputFocus.editor(in: window) {
            window.makeFirstResponder(editor)
        } else if let previousResponder {
            window.makeFirstResponder(previousResponder)
        }
    }

    func detach() {
        terminal.onProcessExit = nil
        terminal.onToggleChat = nil
        active = false
        updateVisibility(focus: false)
        container.removeFromSuperview()
    }

    func dispose() {
        terminal.stop()
    }
}
#endif
