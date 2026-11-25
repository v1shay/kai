import AppKit
import XCTest
@testable import NotchPrototype

@MainActor
final class AdaptiveTextInkCacheTests: XCTestCase {
    private struct Fixture {
        let host: NSView
        let field: NSTextField
        let text: NSTextView
        let layer: CATextLayer
    }

    private func fixture() -> Fixture {
        let host = NSView(frame: NSRect(x: 0, y: 0, width: 500, height: 300))
        host.wantsLayer = true
        let field = NSTextField(labelWithString: "A field")
        field.textColor = NSColor(srgbRed: 0.7, green: 0.7, blue: 0.7, alpha: 0.65)
        let text = NSTextView(frame: host.bounds)
        let content = NSMutableAttributedString(string: "neutral colored link\n", attributes: [
            .foregroundColor: NSColor(srgbRed: 0.8, green: 0.8, blue: 0.8, alpha: 0.7),
            .font: NSFont.systemFont(ofSize: 13)
        ])
        content.addAttribute(.foregroundColor, value: NSColor(srgbRed: 0.9, green: 0.2, blue: 0.4, alpha: 0.5), range: NSRange(location: 8, length: 7))
        content.addAttributes([.link: URL(string: "https://example.com")!, .underlineStyle: NSUnderlineStyle.single.rawValue], range: NSRange(location: 16, length: 4))
        text.textStorage?.setAttributedString(content)
        text.insertionPointColor = .white
        let layer = CATextLayer()
        layer.string = "Layer text"
        layer.foregroundColor = NSColor(srgbRed: 0.2, green: 0.7, blue: 0.4, alpha: 0.8).cgColor
        host.addSubview(field)
        host.addSubview(text)
        host.layer?.addSublayer(layer)
        return Fixture(host: host, field: field, text: text, layer: layer)
    }

    private func equal(_ actual: Fixture, _ expected: Fixture, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(actual.field.textColor, expected.field.textColor, file: file, line: line)
        XCTAssertEqual(actual.text.insertionPointColor, expected.text.insertionPointColor, file: file, line: line)
        XCTAssertEqual(actual.layer.foregroundColor, expected.layer.foregroundColor, file: file, line: line)
        XCTAssertEqual(actual.text.textStorage, expected.text.textStorage, file: file, line: line)
    }

    func testEveryTransitionFrameMatchesLegacyAndIndexesStorageOnlyOnce() {
        let actual = fixture(), expected = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        for frame in 0...120 {
            let progress = min(1, Double(frame) / 60)
            ink.diagnosticApply(color: 0xe3c17f, progress: progress)
            reference.diagnosticApply(color: 0xe3c17f, progress: progress)
            equal(actual, expected)
        }
        XCTAssertEqual(ink.diagnosticStorageBuildCount, 1)
        let writes = ink.diagnosticStorageWriteCount
        ink.diagnosticApply(color: 0xe3c17f, progress: 1)
        XCTAssertEqual(ink.diagnosticStorageWriteCount, writes)
        ink.restore(); reference.restore()
        equal(actual, expected)
    }

    func testReversalHiddenSurfacesAndRestoreMatchLegacy() {
        let actual = fixture(), expected = fixture()
        actual.text.isHidden = true; expected.text.isHidden = true
        actual.layer.isHidden = true; expected.layer.isHidden = true
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        for (color, progress) in [(UInt32(0xe3c17f), 0.0), (0xe3c17f, 0.4), (0x4488bb, 0.0), (0x4488bb, 0.6), (0x4488bb, 1.0)] {
            ink.diagnosticApply(color: color, progress: progress)
            reference.diagnosticApply(color: color, progress: progress)
            equal(actual, expected)
        }
        XCTAssertEqual(ink.diagnosticStorageBuildCount, 2)
        ink.restore(); reference.restore()
        equal(actual, expected)
    }

    func testPartialExternalColorAndCharacterEditsRebuildBeforeApplying() throws {
        let actual = fixture(), expected = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        ink.diagnosticApply(color: 0xdcc08a, progress: 1)
        reference.diagnosticApply(color: 0xdcc08a, progress: 1)
        let a = try XCTUnwrap(actual.text.textStorage), b = try XCTUnwrap(expected.text.textStorage)
        let replacement = NSColor(srgbRed: 0.1, green: 0.4, blue: 0.9, alpha: 0.6)
        a.addAttribute(.foregroundColor, value: replacement, range: NSRange(location: 2, length: 3))
        b.addAttribute(.foregroundColor, value: replacement, range: NSRange(location: 2, length: 3))
        reference.refresh()
        print("INK_DEBUG notifications=\(ink.diagnosticStorageNotificationCount) hostnotifications=\(ink.diagnosticHostStorageNotificationCount) managers=\(a.layoutManagers.count) containers=\(a.layoutManagers.map { $0.textContainers.map { $0.textView != nil } }) descendant=\(actual.text.isDescendant(of: actual.host)) main=\(Thread.isMainThread)")
        equal(actual, expected)
        let suffix = NSAttributedString(string: " tail", attributes: [.foregroundColor: NSColor.white])
        a.append(suffix); b.append(suffix); reference.refresh()
        equal(actual, expected)
        a.replaceCharacters(in: NSRange(location: 1, length: 2), with: "new")
        b.replaceCharacters(in: NSRange(location: 1, length: 2), with: "new")
        reference.refresh()
        equal(actual, expected)
        XCTAssertEqual(ink.diagnosticStorageBuildCount, 4)
        ink.restore(); reference.restore()
        equal(actual, expected)
        XCTAssertEqual(a.attribute(.foregroundColor, at: 4, effectiveRange: nil) as? NSColor, replacement)
    }

    func testNewStorageAndAllDescendantFieldsLayersAreDiscovered() {
        let host = NSView()
        let ink = AdaptiveTextInk(host: host)
        ink.diagnosticApply(color: 0x5588cc, progress: 1)
        let nested = fixture()
        host.addSubview(nested.host)
        nested.text.textStorage?.addAttribute(.kern, value: 0.5, range: NSRange(location: 0, length: 1))
        XCTAssertEqual(ink.diagnosticStorageBuildCount, 1)
        let referenceFixture = fixture()
        referenceFixture.text.textStorage?.addAttribute(.kern, value: 0.5, range: NSRange(location: 0, length: 1))
        let reference = LegacyAdaptiveTextInkReference(host: referenceFixture.host)
        reference.diagnosticApply(color: 0x5588cc, progress: 1)
        equal(nested, referenceFixture)
        ink.restore(); reference.restore()
    }

    func testDetachedStorageEditsInvalidateBeforeReattachment() throws {
        let actual = fixture(), expected = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        ink.diagnosticApply(color: 0xdcc08a, progress: 1)
        reference.diagnosticApply(color: 0xdcc08a, progress: 1)
        actual.text.removeFromSuperview(); expected.text.removeFromSuperview()
        let content = NSAttributedString(string: "Replacement after detaching", attributes: [.foregroundColor: NSColor.red])
        actual.text.textStorage?.setAttributedString(content)
        expected.text.textStorage?.setAttributedString(content)
        actual.host.addSubview(actual.text); expected.host.addSubview(expected.text)
        ink.refresh(); reference.refresh()
        equal(actual, expected)
        XCTAssertEqual(ink.diagnosticStorageBuildCount, 2)
        ink.restore(); reference.restore()
        equal(actual, expected)
    }

    func testUnrelatedStorageEditsDoNotRebuildOrWriteHostStorage() {
        let actual = fixture(), unrelated = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        ink.diagnosticApply(color: 0xdcc08a, progress: 1)
        let builds = ink.diagnosticStorageBuildCount, writes = ink.diagnosticStorageWriteCount
        unrelated.text.textStorage?.addAttribute(.foregroundColor, value: NSColor.red, range: NSRange(location: 0, length: 3))
        XCTAssertEqual(ink.diagnosticStorageBuildCount, builds)
        XCTAssertEqual(ink.diagnosticStorageWriteCount, writes)
        ink.restore()
    }

    func testSharedStorageIsIndexedAndWrittenOnceForMultipleTextViews() {
        let storage = NSTextStorage(string: "Shared", attributes: [.foregroundColor: NSColor.white])
        let host = NSView()
        for _ in 0..<2 {
            let manager = NSLayoutManager(), container = NSTextContainer(size: NSSize(width: 200, height: 100))
            manager.addTextContainer(container); storage.addLayoutManager(manager)
            host.addSubview(NSTextView(frame: NSRect(x: 0, y: 0, width: 200, height: 100), textContainer: container))
        }
        let ink = AdaptiveTextInk(host: host)
        ink.diagnosticApply(color: 0x5588cc, progress: 1)
        XCTAssertEqual(ink.diagnosticStorageBuildCount, 1)
        XCTAssertEqual(ink.diagnosticStorageWriteCount, 1)
        ink.restore()
        XCTAssertEqual(storage.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor, NSColor.white)
    }

    private func drainMainQueue() async {
        let turn = expectation(description: "One main queue turn")
        DispatchQueue.main.async { turn.fulfill() }
        await fulfillment(of: [turn], timeout: 2)
    }

    func testNewFieldAndLayerWithoutStorageEditUseOneCoalescedProducerRefresh() async {
        let host = NSView()
        let ink = AdaptiveTextInk(host: host)
        ink.diagnosticApply(color: 0xaabbcc, progress: 1)
        let builds = ink.diagnosticRefreshCount
        let field = NSTextField(labelWithString: "Inserted after the transition")
        field.textColor = .white
        let layerView = NSView()
        layerView.wantsLayer = true
        let textLayer = CATextLayer()
        textLayer.foregroundColor = NSColor.white.cgColor
        layerView.layer?.addSublayer(textLayer)
        host.addSubview(field); host.addSubview(layerView)
        for _ in 0..<100 { ink.setNeedsRefresh() }
        await drainMainQueue()
        XCTAssertEqual(ink.diagnosticRefreshCount, builds + 1)
        let expected = NSColor(srgbRed: 170.0/255, green: 187.0/255, blue: 204.0/255, alpha: 1)
        XCTAssertEqual(field.textColor, expected)
        XCTAssertEqual(NSColor(cgColor: textLayer.foregroundColor!), expected)
        let settled = ink.diagnosticRefreshCount
        for _ in 0..<4 { await drainMainQueue() }
        XCTAssertEqual(ink.diagnosticRefreshCount, settled)
        ink.restore()
        XCTAssertEqual(field.textColor, NSColor.white)
    }

    func testReentrantPartialForegroundEditInvalidatesCachedRanges() async throws {
        let actual = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let storage = try XCTUnwrap(actual.text.textStorage)
        let external = NSColor(srgbRed: 0.1, green: 0.5, blue: 0.9, alpha: 0.6)
        var changed = false
        let observer = NotificationCenter.default.addObserver(forName: NSTextStorage.didProcessEditingNotification, object: storage, queue: nil) { _ in
            MainActor.assumeIsolated {
                guard !changed else { return }
                changed = true
                storage.addAttribute(.foregroundColor, value: external, range: NSRange(location: 2, length: 2))
            }
        }
        defer { NotificationCenter.default.removeObserver(observer); ink.restore() }
        ink.diagnosticApply(color: 0xaabbcc, progress: 1)
        for _ in 0..<4 { await drainMainQueue() }
        XCTAssertTrue(changed)
        XCTAssertGreaterThanOrEqual(ink.diagnosticStorageBuildCount, 2)
        let expected = fixture()
        expected.text.textStorage?.addAttribute(.foregroundColor, value: external, range: NSRange(location: 2, length: 2))
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        reference.diagnosticApply(color: 0xaabbcc, progress: 1)
        XCTAssertEqual(storage.attribute(.foregroundColor, at: 2, effectiveRange: nil) as? NSColor, expected.text.textStorage?.attribute(.foregroundColor, at: 2, effectiveRange: nil) as? NSColor)
        let refreshes = ink.diagnosticRefreshCount
        for _ in 0..<4 { await drainMainQueue() }
        XCTAssertEqual(ink.diagnosticRefreshCount, refreshes)
        reference.restore()
    }

    func testHiddenWindowActiveFieldEditorRefreshQuiesces() async throws {
        let actual = fixture()
        actual.field.isEditable = true
        actual.field.frame = NSRect(x: 0, y: 0, width: 180, height: 24)
        let window = NSWindow(contentRect: actual.host.bounds, styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = actual.host
        defer { window.endEditing(for: nil); window.contentView = nil; window.close() }
        XCTAssertTrue(window.makeFirstResponder(actual.field))
        actual.field.selectText(nil)
        let editor = try XCTUnwrap(actual.field.currentEditor() as? NSTextView)
        XCTAssertTrue(editor.isFieldEditor)
        let ink = AdaptiveTextInk(host: actual.host)
        ink.apply(color: 0xaabbcc, animated: false)
        for _ in 0..<4 { await drainMainQueue() }
        let refreshes = ink.diagnosticRefreshCount, writes = ink.diagnosticStorageWriteCount
        for _ in 0..<8 { await drainMainQueue() }
        XCTAssertEqual(ink.diagnosticRefreshCount, refreshes)
        XCTAssertEqual(ink.diagnosticStorageWriteCount, writes)
        ink.restore()
    }

    func testAppearanceAndSystemColorInvalidationPreserveLegacyColors() {
        let actual = fixture(), expected = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        ink.diagnosticApply(color: 0xaabbcc, progress: 1)
        reference.diagnosticApply(color: 0xaabbcc, progress: 1)
        let initialBuilds = ink.diagnosticStorageBuildCount
        let alternate: NSAppearance.Name = actual.host.effectiveAppearance.name == .darkAqua ? .aqua : .darkAqua
        actual.host.appearance = NSAppearance(named: alternate)
        expected.host.appearance = NSAppearance(named: alternate)
        ink.refresh(); reference.refresh()
        equal(actual, expected)
        XCTAssertEqual(ink.diagnosticStorageBuildCount, initialBuilds + 1)
        NotificationCenter.default.post(name: NSColor.systemColorsDidChangeNotification, object: nil)
        reference.refresh()
        equal(actual, expected)
        XCTAssertEqual(ink.diagnosticStorageBuildCount, initialBuilds + 2)
        ink.restore(); reference.restore()
    }

    private func verifyReentrantCommittedMutation(_ mode: String, file: StaticString = #filePath, line: UInt = #line) async throws {
        let actual = fixture(), expected = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        let storage = try XCTUnwrap(actual.text.textStorage)
        func mutate(_ storage: NSTextStorage) {
            switch mode {
            case "append":
                storage.append(NSAttributedString(string: " appended!", attributes: [.foregroundColor: NSColor.green]))
            case "delete":
                storage.deleteCharacters(in: NSRange(location: 1, length: 2))
            default:
                let replacement = NSMutableAttributedString(attributedString: storage)
                for name in ["kaiTextOriginal", "kaiTextApplied", "kaiTextFrom", "kaiTextEpoch"] {
                    replacement.removeAttribute(NSAttributedString.Key(name), range: NSRange(location: 0, length: replacement.length))
                }
                storage.setAttributedString(replacement)
            }
        }
        var changed = false
        let observer = NotificationCenter.default.addObserver(forName: NSTextStorage.didProcessEditingNotification, object: storage, queue: nil) { _ in
            MainActor.assumeIsolated {
                guard !changed else { return }
                changed = true
                mutate(storage)
            }
        }
        defer { NotificationCenter.default.removeObserver(observer); ink.restore(); reference.restore() }
        ink.diagnosticApply(color: 0xaabbcc, progress: 0.25)
        reference.diagnosticApply(color: 0xaabbcc, progress: 0.25)
        mutate(try XCTUnwrap(expected.text.textStorage))
        reference.refresh()
        for _ in 0..<4 { await drainMainQueue() }
        XCTAssertTrue(changed, file: file, line: line)
        XCTAssertEqual(actual.text.textStorage, expected.text.textStorage, file: file, line: line)
        XCTAssertGreaterThanOrEqual(ink.diagnosticStorageBuildCount, 2, file: file, line: line)
        let refreshes = ink.diagnosticRefreshCount
        for _ in 0..<4 { await drainMainQueue() }
        XCTAssertEqual(ink.diagnosticRefreshCount, refreshes, file: file, line: line)
    }

    func testReentrantAppendDuringOwnEditingMatchesLegacyAndQuiesces() async throws {
        try await verifyReentrantCommittedMutation("append")
    }

    func testReentrantDeletionDuringOwnEditingMatchesLegacyAndQuiesces() async throws {
        try await verifyReentrantCommittedMutation("delete")
    }

    func testReentrantSameLengthAttributedReplacementMatchesLegacyAndQuiesces() async throws {
        try await verifyReentrantCommittedMutation("replacement")
    }

    func testRestoreDoesNotEditNewUninkedStorage() throws {
        let actual = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        ink.diagnosticApply(color: 0xaabbcc, progress: 1)
        let fresh = NSTextView(frame: actual.host.bounds)
        let storage = try XCTUnwrap(fresh.textStorage)
        storage.setAttributedString(NSAttributedString(string: "Fresh original", attributes: [.foregroundColor: NSColor.red]))
        let original = NSAttributedString(attributedString: storage)
        actual.host.addSubview(fresh)
        var edits = 0
        let observer = NotificationCenter.default.addObserver(forName: NSTextStorage.didProcessEditingNotification, object: storage, queue: nil) { _ in edits += 1 }
        defer { NotificationCenter.default.removeObserver(observer) }
        ink.restore()
        XCTAssertEqual(edits, 0)
        XCTAssertEqual(storage, original)
        XCTAssertNil(actual.layer.value(forKey: "kaiTextDestination"))
    }

    func testImmediateReducedMotionStyleApplicationAndRestorePreserveOtherAttributes() {
        let actual = fixture(), expected = fixture()
        let ink = AdaptiveTextInk(host: actual.host)
        let reference = LegacyAdaptiveTextInkReference(host: expected.host)
        ink.apply(color: 0xaabbcc, animated: false)
        reference.apply(color: 0xaabbcc, animated: false)
        equal(actual, expected)
        ink.restore(); reference.restore()
        equal(actual, expected)
        XCTAssertNotNil(actual.text.textStorage?.attribute(.link, at: 16, effectiveRange: nil))
    }
}
