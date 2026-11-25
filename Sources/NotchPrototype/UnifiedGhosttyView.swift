#if canImport(libghostty)
import AppKit
import Darwin
import libghostty
import Metal

private func terminalShellCommand(_ arguments: [String]) -> String {
    (["/bin/sh"] + arguments).map {
        "'" + $0.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
    }.joined(separator: " ")
}

private final class TerminalContainer: NSView {
    private var retiring = false

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard !retiring else { return nil }
        let hit = super.hitTest(point)
        return hit === self ? nil : hit
    }

    func retire() {
        retiring = true
        guard !isHidden, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else {
            removeFromSuperview()
            return
        }
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = KaiMotion.paneDuration
            context.timingFunction = KaiMotion.interactionCurve
            animator().alphaValue = 0
        }, completionHandler: { [self] in removeFromSuperview() })
    }
}

@MainActor
private final class WorkspaceGhosttyRuntime {
    static let shared = WorkspaceGhosttyRuntime()
    static let hasMetalDevice = MTLCreateSystemDefaultDevice() != nil
    private(set) var app: ghostty_app_t?
    private final class WakeGate: @unchecked Sendable {
        private let lock = NSLock()
        private var pending = false

        func request() {
            lock.lock()
            if pending { lock.unlock(); return }
            pending = true
            lock.unlock()
            DispatchQueue.main.async { [self] in
                lock.lock()
                pending = false
                lock.unlock()
                if let app = WorkspaceGhosttyRuntime.shared.app { ghostty_app_tick(app) }
            }
        }
    }
    private nonisolated static let wakeGate = WakeGate()

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
            WorkspaceGhosttyRuntime.wakeGate.request()
        }
        callbacks.action_cb = { _, target, action in
            guard target.tag == GHOSTTY_TARGET_SURFACE else { return false }
            let surface = target.target.surface
            if action.tag == GHOSTTY_ACTION_RENDER {
                ghostty_surface_draw(surface)
                return true
            }
            if action.tag == GHOSTTY_ACTION_SET_TITLE,
               let pointer = ghostty_surface_userdata(surface),
               let title = action.action.set_title.title {
                let view = Unmanaged<WorkspaceGhosttyView>.fromOpaque(pointer).takeUnretainedValue()
                let value = String(cString: title)
                DispatchQueue.main.async { [weak view] in view?.onTitleChange?(value) }
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


}

final class WorkspaceGhosttyView: NSView, NSTextInputClient {
    private var surface: ghostty_surface_t?
    private var requestedRenderingVisible = true
    private var appliedRenderingVisible: Bool?
    private var visibilityObservers: [NSObjectProtocol] = []
    var onRenderingVisibilityChange: (() -> Void)?
    var isRenderingVisible: Bool { appliedRenderingVisible == true }
    private var appliedScale: CGFloat = 0
    private var appliedWidth: UInt32 = 0
    private var appliedHeight: UInt32 = 0
    private var preedit = NSAttributedString()
    private var preeditSelection = NSRange(location: NSNotFound, length: 0)
    private var keyText: String?
    var onFocus: (() -> Void)?
    var onProcessExit: (() -> Void)?
    var onTitleChange: ((String) -> Void)?

    var inputBlocked = false
    override var acceptsFirstResponder: Bool { !inputBlocked }
    override func hitTest(_ point: NSPoint) -> NSView? { inputBlocked ? nil : super.hitTest(point) }
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
        guard surface != nil else { return false }
        updateSize()
        appliedRenderingVisible = nil
        updateRenderingVisibility()
        return true
    }

    func foregroundProgram() -> String? {
        guard let surface,
              let pid = pid_t(exactly: ghostty_surface_foreground_pid(surface)), pid > 0 else { return nil }
        var buffer = [UInt8](repeating: 0, count: 256)
        let length = buffer.withUnsafeMutableBytes { bytes in
            proc_name(pid, bytes.baseAddress, UInt32(bytes.count))
        }
        guard length > 0 else { return nil }
        return String(decoding: buffer.prefix(Int(length)).prefix { $0 != 0 }, as: UTF8.self)
    }

    func setRenderingVisible(_ visible: Bool) {
        requestedRenderingVisible = visible
        updateRenderingVisibility()
    }

    private func updateRenderingVisibility() {
        let visible = requestedRenderingVisible && !isHiddenOrHasHiddenAncestor
            && window?.isVisible == true && window?.isMiniaturized == false
            && window?.occlusionState.contains(.visible) == true
        guard let surface, appliedRenderingVisible != visible else { return }
        appliedRenderingVisible = visible
        ghostty_surface_set_occlusion(surface, visible)
        if visible { ghostty_surface_refresh(surface) }
        onRenderingVisibilityChange?()
    }

    func stop() {
        guard let surface else { return }
        self.surface = nil
        appliedRenderingVisible = nil
        ghostty_surface_free(surface)
    }

    deinit {
        visibilityObservers.forEach { NotificationCenter.default.removeObserver($0) }
        if let surface { ghostty_surface_free(surface) }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        visibilityObservers.forEach { NotificationCenter.default.removeObserver($0) }
        visibilityObservers.removeAll()
        if let window {
            for name in [NSWindow.didChangeOcclusionStateNotification,
                         NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification] {
                visibilityObservers.append(NotificationCenter.default.addObserver(
                    forName: name, object: window, queue: .main
                ) { [weak self] _ in self?.updateRenderingVisibility() })
            }
        }
        updateRenderingVisibility()
        updateSize()
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        updateSize()
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if surface != nil { queueGridUpdate() }
    }

    // A live resize can issue several AppKit geometry updates before paint.
    // Keep the view geometry immediate and apply the latest grid once per turn.
    private var gridUpdateQueued = false
    private func queueGridUpdate() {
        guard !gridUpdateQueued else { return }
        gridUpdateQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.gridUpdateQueued = false
            self.updateSize()
        }
    }

    private func updateSize() {
        guard let surface, let window, bounds.width > 0, bounds.height > 0 else { return }
        let scale = window.backingScaleFactor
        let width = UInt32(bounds.width * scale)
        let height = UInt32(bounds.height * scale)
        if appliedScale != scale {
            appliedScale = scale
            ghostty_surface_set_content_scale(surface, scale, scale)
        }
        if appliedWidth != width || appliedHeight != height {
            appliedWidth = width
            appliedHeight = height
            ghostty_surface_set_size(surface, width, height)
        }
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
        guard !inputBlocked, window?.firstResponder === self else { return false }
        let flags = event.modifierFlags.intersection([.command, .shift, .control, .option])
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

    func handleWorkspaceArrow(_ event: NSEvent) -> Bool {
        guard !inputBlocked, surface != nil, !hasMarkedText(),
              WorkspaceArrowKey(event) != nil else { return false }
        // Route cursor keys before AppKit's text services can turn them into
        // legacy control characters via insertText.
        if event.type == .keyDown {
            let flags = event.modifierFlags.intersection([.command, .shift, .control, .option])
            if flags == .command, performKeyEquivalent(with: event) { return true }
            keyDown(with: event)
        } else {
            keyUp(with: event)
        }
        return true
    }

    override func keyDown(with event: NSEvent) {
        guard !inputBlocked else { return }
        guard let surface else { return }
        let translation = ghostty_surface_key_translation_mods(surface, mods(event.modifierFlags))
        let translated = translationEvent(event, using: translation)
        if WorkspaceArrowKey(event) != nil, !hasMarkedText() {
            let action = event.isARepeat ? GHOSTTY_ACTION_REPEAT : GHOSTTY_ACTION_PRESS
            sendKey(event, action: action, translatedFlags: translated.modifierFlags)
            return
        }
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
        guard !inputBlocked else { return }
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
        guard !inputBlocked, let surface else { return }
        let arrow = WorkspaceArrowKey(event)
        // AppKit's legacy arrow characters are C0 controls. Keep them out
        // of the text fields and let Ghostty encode the physical cursor key.
        let codepoint = arrow == nil
            ? event.characters(byApplyingModifiers: [])?.unicodeScalars.first?.value ?? 0
            : 0
        var key = ghostty_input_key_s(action: action, mods: mods(event.modifierFlags),
                                     consumed_mods: mods(translatedFlags.subtracting([.control, .command])),
                                     keycode: UInt32(arrow?.rawValue ?? event.keyCode), text: nil,
                                     unshifted_codepoint: codepoint, composing: hasMarkedText())
        if let text, !text.isEmpty {
            text.withCString { key.text = $0; _ = ghostty_surface_key(surface, key) }
        } else {
            _ = ghostty_surface_key(surface, key)
        }
    }

    private func sendCommittedText(_ text: String) {
        guard !inputBlocked, let surface else { return }
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
        guard !inputBlocked, let surface else { return }
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
        guard !inputBlocked else { return }
        window?.makeFirstResponder(self)
        mouse(event, state: GHOSTTY_MOUSE_PRESS, button: GHOSTTY_MOUSE_LEFT)
    }

    override func mouseUp(with event: NSEvent) { mouse(event, state: GHOSTTY_MOUSE_RELEASE, button: GHOSTTY_MOUSE_LEFT) }
    override func rightMouseDown(with event: NSEvent) { mouse(event, state: GHOSTTY_MOUSE_PRESS, button: GHOSTTY_MOUSE_RIGHT) }
    override func rightMouseUp(with event: NSEvent) { mouse(event, state: GHOSTTY_MOUSE_RELEASE, button: GHOSTTY_MOUSE_RIGHT) }
    override func mouseDragged(with event: NSEvent) { mouseMoved(with: event) }

    override func mouseMoved(with event: NSEvent) {
        guard !inputBlocked, let surface else { return }
        let point = convert(event.locationInWindow, from: nil)
        ghostty_surface_mouse_pos(surface, point.x, bounds.height - point.y, mods(event.modifierFlags))
    }

    private func mouse(_ event: NSEvent, state: ghostty_input_mouse_state_e, button: ghostty_input_mouse_button_e) {
        guard !inputBlocked, let surface else { return }
        mouseMoved(with: event)
        _ = ghostty_surface_mouse_button(surface, state, button, mods(event.modifierFlags))
    }

    override func scrollWheel(with event: NSEvent) {
        guard !inputBlocked, let surface else { return }
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
        guard !inputBlocked, let surface else { return }
        text.withCString { ghostty_surface_text(surface, $0, UInt(text.utf8.count)) }
    }

    func probeInput(_ value: String) -> Bool {
        guard surface != nil, let window else { return false }
        let command = value.hasSuffix("\r") ? String(value.dropLast()) : value
        input(command)
        guard value.hasSuffix("\r") else { return true }
        guard let enter = NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil,
            characters: "\r", charactersIgnoringModifiers: "\r",
            isARepeat: false, keyCode: 36) else { return false }
        sendKey(enter, action: GHOSTTY_ACTION_PRESS, translatedFlags: [])
        sendKey(enter, action: GHOSTTY_ACTION_RELEASE, translatedFlags: [])
        return true
    }

    func probeContains(_ marker: String) -> Bool {
        guard let surface else { return false }
        let selection = ghostty_selection_s(
            top_left: ghostty_point_s(tag: GHOSTTY_POINT_VIEWPORT, coord: GHOSTTY_POINT_COORD_TOP_LEFT, x: 0, y: 0),
            bottom_right: ghostty_point_s(tag: GHOSTTY_POINT_VIEWPORT, coord: GHOSTTY_POINT_COORD_BOTTOM_RIGHT, x: 0, y: 0),
            rectangle: false
        )
        var text = ghostty_text_s()
        guard ghostty_surface_read_text(surface, selection, &text) else { return false }
        defer { ghostty_surface_free_text(surface, &text) }
        guard let bytes = text.text else { return false }
        return String(decoding: Data(bytes: bytes, count: Int(text.text_len)), as: UTF8.self).contains(marker)
    }

    @objc func copy(_ sender: Any?) { copySelection() }
    @objc func paste(_ sender: Any?) { paste() }
    override func selectAll(_ sender: Any?) {
        guard !inputBlocked, let surface else { return }
        let action = "select_all"
        action.withCString { _ = ghostty_surface_binding_action(surface, $0, UInt(action.utf8.count)) }
    }

    func copySelection() {
        guard !inputBlocked, let surface else { return }
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
    private var inputBlocked = false
    func setInputBlocked(_ blocked: Bool) {
        inputBlocked = blocked
        for (id, surface) in surfaces {
            surface.terminal.inputBlocked = blocked
            if blocked { hide(id: id) }
        }
    }
    @MainActor
    private final class Surface {
        let clipView = TerminalContainer()
        let terminal = WorkspaceGhosttyView()
        var requestedFocus = false
        var animateResizeUntil: TimeInterval = 0
        var foregroundProgram: String?
    }

    private var surfaces: [String: Surface] = [:]
    private var closing = Set<String>()
    private var foregroundTimer: Timer?

    private func updateForegroundTimer() {
        guard surfaces.values.contains(where: { $0.terminal.isRenderingVisible }) else {
            foregroundTimer?.invalidate()
            foregroundTimer = nil
            return
        }
        guard foregroundTimer == nil else { return }
        let timer = Timer(timeInterval: 0.4, repeats: true) { [weak self] timer in
            MainActor.assumeIsolated {
                guard let self else { timer.invalidate(); return }
                for (id, surface) in self.surfaces where surface.terminal.isRenderingVisible {
                    let program = surface.terminal.foregroundProgram()
                    if surface.foregroundProgram != program,
                       EmbeddedCodeRuntime.shared.sendAction("dock_terminal_program", [
                           "id": id, "program": program ?? ""
                       ]) {
                        surface.foregroundProgram = program
                    }
                }
            }
        }
        timer.tolerance = 0.1
        RunLoop.main.add(timer, forMode: .common)
        foregroundTimer = timer
    }

    func animateNextResize(id: String) {
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
              let surface = surfaces[id], !surface.clipView.isHidden else { return }
        surface.animateResizeUntil = CACurrentMediaTime() + KaiMotion.paneDuration
    }

    func update(id: String, directory: String?, command: String?, rect: CGRect, clip: CGRect, focused: Bool, window: NSWindow) {
        guard !closing.contains(id), let parent = window.contentView else { return }
        let surface: Surface
        if let existing = surfaces[id] {
            surface = existing
        } else {
            guard directory != nil else { return }
            surface = Surface()
            surface.terminal.onRenderingVisibilityChange = { [weak self] in self?.updateForegroundTimer() }
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
        let clipChanged = surface.clipView.frame != frame
        let terminalChanged = surface.terminal.frame != terminalFrame
        if (clipChanged || terminalChanged) && directory == nil &&
            CACurrentMediaTime() < surface.animateResizeUntil {
            surface.animateResizeUntil = 0
            NSAnimationContext.runAnimationGroup { context in
                context.duration = KaiMotion.paneDuration
                context.timingFunction = KaiMotion.interactionCurve
                if clipChanged { surface.clipView.animator().frame = frame }
                if terminalChanged { surface.terminal.animator().frame = terminalFrame }
            }
        } else {
            if clipChanged { surface.clipView.frame = frame }
            if terminalChanged { surface.terminal.frame = terminalFrame }
        }
        if let directory {
            DispatchQueue.main.async { [weak self, weak surface, weak window] in
                guard let self, let surface, let window, self.surfaces[id] === surface else { return }
                surface.terminal.onProcessExit = { [weak self] in self?.close(id: id) }
                surface.terminal.onTitleChange = { title in
                    EmbeddedCodeRuntime.shared.sendAction("dock_terminal_title", ["id": id, "title": String(title.prefix(160))])
                }
                let launch = command.map { terminalShellCommand(["-lc", $0]) }
                guard surface.terminal.start(directory: URL(fileURLWithPath: directory, isDirectory: true),
                                             command: launch, waitAfterCommand: false) else {
                    self.close(id: id, error: "Could not start Ghostty Metal. Check the terminal’s working directory.")
                    return
                }
                surface.terminal.setRenderingVisible(!surface.clipView.isHidden)
                if !surface.clipView.isHidden, surface.requestedFocus, window.isKeyWindow {
                    window.makeFirstResponder(surface.terminal)
                }
                EmbeddedCodeRuntime.shared.sendAction("dock_terminal_ready", ["id": id])
            }
        }
        surface.terminal.inputBlocked = inputBlocked
        if inputBlocked { hide(id: id); return }
        if surface.clipView.isHidden {
            surface.clipView.isHidden = false
            surface.terminal.setRenderingVisible(true)
        }
        if directory == nil, focused, !surface.requestedFocus, window.isKeyWindow {
            window.makeFirstResponder(surface.terminal)
        }
        surface.requestedFocus = focused
        updateForegroundTimer()
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
        updateForegroundTimer()
    }

    func close(id: String, error: String? = nil) {
        guard let surface = surfaces[id] else { return }
        surfaces.removeValue(forKey: id)
        updateForegroundTimer()
        if let window = surface.terminal.window, window.firstResponder === surface.terminal,
           let editor = WorkspaceInputFocus.editor(in: window) {
            window.makeFirstResponder(editor)
        }
        surface.terminal.onProcessExit = nil
        surface.terminal.onTitleChange = nil
        surface.terminal.setRenderingVisible(false)
        surface.clipView.retire()
        closing.insert(id)
        let delay = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : KaiMotion.paneDuration
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [self] in
            surface.terminal.stop()
            closing.remove(id)
            var fields: [String: Any] = ["id": id]
            if let error { fields["error"] = error }
            EmbeddedCodeRuntime.shared.sendAction("dock_terminal_closed", fields)
        }
    }

    func focusVisibleTerminalForProbe(in window: NSWindow) -> Bool {
        let visible = surfaces.values.filter { !$0.clipView.isHidden && $0.terminal.window === window }
        guard let surface = visible.first(where: { $0.requestedFocus }) ?? visible.first else { return false }
        window.makeKey()
        return window.makeFirstResponder(surface.terminal) && window.firstResponder === surface.terminal
    }

    func probe(id: String, input: Bool, value: String) -> Bool {
        guard let surface = surfaces[id], !surface.clipView.isHidden else { return false }
        if input { return surface.terminal.probeInput(value) }
        return surface.terminal.probeContains(value)
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
    private var inputBlocked = false
    func setInputBlocked(_ blocked: Bool) {
        inputBlocked = blocked
        for (owner, surface) in surfaces { update(surface, owner: owner, focus: false) }
    }
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
        surface.terminal.inputBlocked = inputBlocked
        let visible = !inputBlocked && (owner == activeChat || peerChats.contains(owner)) && bounds.width > 0 && bounds.height > 44
        let focus = visible && surface.isSelected && (focus || pendingFocus == owner)
        if focus { pendingFocus = nil }
        surface.setActive(visible, bounds: bounds, focus: focus)
    }

    func ownsResponder(_ responder: WorkspaceGhosttyView) -> Bool {
        surfaces.values.contains { $0.ownsResponder(responder) }
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

    func hasSurface(id: String, launchId: String) -> Bool {
        surfaces[id]?.launchId == launchId
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
        let delay = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : KaiMotion.paneDuration
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [self] in
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
    private let container: TerminalContainer
    fileprivate let terminal: WorkspaceGhosttyView
    private var active = false
    private var selected = true
    var isSelected: Bool { selected }
    var onExit: (() -> Void)?

    init?(id: String, launchId: String, arguments: [String], directory: String, title: String, agent: String,
          environment: [[String]], bounds: CGRect, window: NSWindow) {
        guard let content = window.contentView else { return nil }
        self.id = id
        self.launchId = launchId
        self.window = window
        container = TerminalContainer()
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
        container.isHidden = true
        content.addSubview(container)
        terminal.onFocus = { [weak self] in
            guard let self else { return }
            if self.previousResponder == nil, let window = self.window {
                self.previousResponder = WorkspaceInputFocus.editor(in: window)
            }
            EmbeddedCodeRuntime.shared.sendAction("chat_terminal_focus", ["id": self.id, "launchId": self.launchId])
        }
        terminal.onProcessExit = { [weak self] in self?.onExit?() }
        let command = terminalShellCommand(arguments)
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

    func setSelected(_ selected: Bool, focus: Bool) {
        self.selected = selected
        updateVisibility(focus: focus)
    }

    private func updateVisibility(focus: Bool) {
        let visible = active && selected && !terminal.inputBlocked
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
        active = false
        restoreEditorFocus()
        terminal.setRenderingVisible(false)
        container.retire()
    }

    func dispose() {
        terminal.stop()
    }
}
#endif
