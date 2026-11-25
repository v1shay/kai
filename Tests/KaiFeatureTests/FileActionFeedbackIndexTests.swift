import AppKit
import XCTest
@testable import NotchPrototype

private final class FeedbackLayoutManager: NSLayoutManager {
    var invalidatedFeedbackRanges = [NSRange]()
    override func invalidateDisplay(forCharacterRange range: NSRange) {
        invalidatedFeedbackRanges.append(range)
        super.invalidateDisplay(forCharacterRange: range)
    }
}

final class FileActionFeedbackIndexTests: XCTestCase {
    private let url = URL(fileURLWithPath: "/tmp/kai-feedback-fixture.txt")

    @MainActor private func button(_ url: URL, action: String = "copy") -> (NSAttributedString, NSTextAttachment) {
        let attachment = NSTextAttachment()
        attachment.image = NSImage(size: CGSize(width: 24, height: 22))
        let value = NSMutableAttributedString(attachment: attachment)
        value.addAttribute(.link, value: FileActionSymbols.link(action, url: url)!, range: NSRange(location: 0, length: value.length))
        return (value, attachment)
    }

    @MainActor private func view(_ content: NSAttributedString) -> NSTextView {
        let view = NSTextView(frame: CGRect(x: 0, y: 0, width: 400, height: 200))
        view.textStorage!.setAttributedString(content)
        return view
    }

    @MainActor func testLargeColdTranscriptBuildsOneIndexAcrossThirtyFeedbackTicks() {
        let content = NSMutableAttributedString(string: "")
        for index in 0..<1500 {
            let line = NSMutableAttributedString(string: String(repeating: "Transcript content ", count: 4) + "\n")
            line.addAttribute(.link, value: URL(string: "https://example.invalid/\(index)")!, range: NSRange(location: 0, length: line.length))
            content.append(line)
        }
        let (first, a) = button(url), (second, b) = button(url)
        content.append(first); content.append(second)
        let text = view(content), renderer = ConversationRenderer(mathRenderer: MathRenderer())
        renderer.refreshFileAction("copy", url: url, in: text)
        let scanned = renderer.debugFileActionVisitedCharacters
        XCTAssertGreaterThan(scanned, 100_000)
        for _ in 0..<30 { renderer.refreshFileAction("copy", url: url, in: text) }
        XCTAssertEqual(renderer.debugFileActionVisitedCharacters, scanned)
        XCTAssertTrue(a.image === b.image, "Duplicate targets must reuse the exact same progress image")
    }

    @MainActor func testCoalescedInsertionsShiftDuplicateTargetsAndScanOnlyCommittedEdit() {
        let content = NSMutableAttributedString(string: "prefix ")
        let (first, a) = button(url), (second, b) = button(url)
        content.append(first); content.append(NSAttributedString(string: " middle ")); content.append(second)
        let text = view(content), storage = text.textStorage!, renderer = ConversationRenderer(mathRenderer: MathRenderer())
        renderer.refreshFileAction("copy", url: url, in: text)
        let scanned = renderer.debugFileActionVisitedCharacters
        storage.beginEditing()
        storage.insert(NSAttributedString(string: "new "), at: 0)
        storage.insert(NSAttributedString(string: "more "), at: 4)
        storage.endEditing()
        a.image = nil; b.image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNotNil(a.image); XCTAssertTrue(a.image === b.image)
        XCTAssertLessThanOrEqual(renderer.debugFileActionVisitedCharacters - scanned, 9)
        storage.deleteCharacters(in: NSRange(location: 0, length: 9))
        a.image = nil; b.image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNotNil(a.image); XCTAssertTrue(a.image === b.image)
        let (replacement, current) = button(url)
        storage.beginEditing()
        storage.insert(NSAttributedString(string: "!"), at: 0)
        let last = (storage.string as NSString).range(of: "\u{fffc}", options: .backwards)
        storage.replaceCharacters(in: last, with: replacement)
        storage.endEditing()
        a.image = nil; b.image = nil; current.image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNotNil(a.image); XCTAssertTrue(a.image === current.image)
        XCTAssertNil(b.image, "A replaced duplicate must retire from the index")
    }

    @MainActor func testAttributeOnlyRelinkUpdatesExactURLMembership() {
        let other = URL(fileURLWithPath: "/tmp/other-feedback-fixture.txt")
        let (value, attachment) = button(url)
        let text = view(value), storage = text.textStorage!, renderer = ConversationRenderer(mathRenderer: MathRenderer())
        renderer.refreshFileAction("copy", url: url, in: text)
        storage.addAttribute(.link, value: FileActionSymbols.link("copy", url: other)!, range: NSRange(location: 0, length: 1))
        attachment.image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNil(attachment.image)
        renderer.refreshFileAction("copy", url: other, in: text)
        XCTAssertNotNil(attachment.image)
    }

    @MainActor func testReplacementRemovalClearAndColdStorageRebinding() {
        let (value, old) = button(url)
        let text = view(value), storage = text.textStorage!, renderer = ConversationRenderer(mathRenderer: MathRenderer())
        renderer.refreshFileAction("copy", url: url, in: text)
        let (replacement, current) = button(url)
        storage.replaceCharacters(in: NSRange(location: 0, length: storage.length), with: replacement)
        old.image = nil; current.image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNil(old.image); XCTAssertNotNil(current.image)
        storage.setAttributedString(NSAttributedString(string: ""))
        current.image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNil(current.image)
        let (next, nextAttachment) = button(url), nextView = view(next)
        renderer.refreshFileAction("copy", url: url, in: nextView)
        XCTAssertNotNil(nextAttachment.image)
    }

    @MainActor func testExistingRendererCommitAndStreamingReplacementKeepIndexCurrent() throws {
        let text = view(NSAttributedString(string: "")), scroll = NSScrollView()
        scroll.documentView = text
        let renderer = ConversationRenderer(mathRenderer: MathRenderer())
        func message(_ id: String, _ path: String) throws -> KaiMessage {
            let row: [String: Any] = ["id": id, "kind": "assistant", "lines": [["text": path, "style": "interaction:tool:document", "links": [["location": 0, "length": (path as NSString).length, "target": URL(fileURLWithPath: path).absoluteString]]]]]
            return try JSONDecoder().decode(KaiMessage.self, from: JSONSerialization.data(withJSONObject: row))
        }
        func targets(_ target: URL) -> [NSTextAttachment] {
            var result = [NSTextAttachment]()
            let storage = text.textStorage!
            storage.enumerateAttribute(.link, in: NSRange(location: 0, length: storage.length)) { value, range, _ in
                guard value as? URL == FileActionSymbols.link("copy", url: target) else { return }
                storage.enumerateAttribute(.attachment, in: range) { value, _, _ in
                    if let attachment = value as? NSTextAttachment { result.append(attachment) }
                }
            }
            return result
        }
        let a = try message("one", url.path), b = try message("two", url.path)
        _ = renderer.render([a,b], threadID: "one", in: text, scroll: scroll, renderMath: false, palette: [.white])
        renderer.refreshFileAction("copy", url: url, in: text)
        let original = targets(url)
        XCTAssertEqual(original.count, 2)
        XCTAssertTrue(original[0].image === original[1].image)
        let scanned = renderer.debugFileActionVisitedCharacters
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertEqual(renderer.debugFileActionVisitedCharacters, scanned)
        let c = try message("older", "/tmp/older-feedback.txt")
        _ = renderer.render([c,a,b], threadID: "one", in: text, scroll: scroll, renderMath: false, palette: [.white])
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertEqual(targets(url).map(ObjectIdentifier.init), original.map(ObjectIdentifier.init))
        _ = renderer.render([c,a], threadID: "one", in: text, scroll: scroll, renderMath: false, palette: [.white])
        original[1].image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertEqual(targets(url).count, 1)
        XCTAssertNil(original[1].image)
        let movedURL = URL(fileURLWithPath: "/tmp/moved-feedback.txt")
        let moved = try message("one", movedURL.path)
        _ = renderer.render([c,moved], threadID: "one", in: text, scroll: scroll, renderMath: false, palette: [.white])
        original[0].image = nil
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertNil(original[0].image)
        renderer.refreshFileAction("copy", url: movedURL, in: text)
        XCTAssertEqual(targets(movedURL).count, 1)
        XCTAssertNotNil(targets(movedURL).first?.image)
        _ = renderer.render([], threadID: "other", in: text, scroll: scroll, renderMath: false, palette: [.white])
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertEqual(text.string, "")
    }

    @MainActor func testClearRemovesIndexedAttachmentReferences() {
        let renderer = ConversationRenderer(mathRenderer: MathRenderer())
        let (value, _) = button(url)
        let text = view(value)
        renderer.refreshFileAction("copy", url: url, in: text)
        text.textStorage!.setAttributedString(NSAttributedString(string: ""))
        XCTAssertEqual(renderer.debugFileActionIndexedTargetCount, 0)
        // AppKit's attributed-string caches may independently retain the old
        // attachment. The index must retire its own URL/range records now.
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertEqual(renderer.debugFileActionIndexedTargetCount, 0)
    }

    @MainActor func testInteriorInsertionInMultiCharacterAttachmentRunInvalidatesFinalCharacter() {
        let attachment = NSTextAttachment()
        let content = NSMutableAttributedString(string: "\u{FFFC}\u{FFFC}", attributes: [
            .attachment: attachment, .link: FileActionSymbols.link("copy", url: url)!
        ])
        let storage = NSTextStorage(attributedString: content)
        let manager = FeedbackLayoutManager()
        let container = NSTextContainer(size: CGSize(width: 300, height: 200))
        manager.addTextContainer(container); storage.addLayoutManager(manager)
        let text = NSTextView(frame: CGRect(x: 0, y: 0, width: 300, height: 200), textContainer: container)
        let renderer = ConversationRenderer(mathRenderer: MathRenderer())
        renderer.refreshFileAction("copy", url: url, in: text)
        storage.insert(NSAttributedString(string: "Z"), at: 1)
        manager.invalidatedFeedbackRanges.removeAll()
        renderer.refreshFileAction("copy", url: url, in: text)
        XCTAssertEqual(storage.length, 3)
        XCTAssertTrue(manager.invalidatedFeedbackRanges.contains { NSLocationInRange(2, $0) },
                      "The shifted final attachment character must be redrawn")
    }

    @MainActor func testStaticIngredientsStayBoundedAcrossProgressAndSizes() {
        for padded in [false, true] {
            for action in ["copy", "reveal"] {
                for step in 0..<50 { _ = FileActionSymbols.image(action, progress: CGFloat(step)/49, padded: padded) }
            }
        }
        XCTAssertLessThanOrEqual(FileActionSymbols.debugIngredientBuilds, 3)
    }

    @MainActor private func pixels(_ image: NSImage, padded: Bool) -> Data {
        let width = padded ? 24 : 12, height = padded ? 22 : 12
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width*2, pixelsHigh: height*2, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: width*8, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        NSGraphicsContext.current?.cgContext.clear(CGRect(x: 0, y: 0, width: width*2, height: height*2))
        image.draw(in: NSRect(x: 0, y: 0, width: width*2, height: height*2), from: .zero, operation: .copy, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        return Data(bytes: bitmap.bitmapData!, count: bitmap.bytesPerRow*bitmap.pixelsHigh)
    }

    @MainActor private func legacy(_ action: String, progress: CGFloat, padded: Bool) -> NSImage {
        let finder = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.finder").map { NSWorkspace.shared.icon(forFile: $0.path) }
        let original = (action == "reveal" ? finder : nil) ?? NSImage(systemSymbolName: action == "reveal" ? "folder" : "doc.on.clipboard", accessibilityDescription: action == "reveal" ? "Show in Finder" : "Copy file")?.withSymbolConfiguration(.init(paletteColors: [.white.withAlphaComponent(0.8)]))
        let check = NSImage(systemSymbolName: "checkmark", accessibilityDescription: "Done")?.withSymbolConfiguration(.init(paletteColors: [.systemGreen]))
        let size = padded ? CGSize(width: 24, height: 22) : CGSize(width: 12, height: 12)
        return NSImage(size: size, flipped: false) { _ in
            let rect = CGRect(x: (size.width-12)/2, y: (size.height-12)/2, width: 12, height: 12)
            original?.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1-progress)
            check?.draw(in: rect, from: .zero, operation: .sourceOver, fraction: progress)
            return true
        }
    }

    @MainActor func testContinuousFeedbackPixelsMatchLegacyAcrossAppearanceAndSizes() {
        for name in [NSAppearance.Name.aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua] {
            NSAppearance(named: name)!.performAsCurrentDrawingAppearance {
                for padded in [false,true] {
                    for action in ["copy","reveal"] {
                        for progress: CGFloat in [0,0.0001,0.125,0.333,0.5,0.75,0.999,1] {
                            let expected = pixels(legacy(action, progress: progress, padded: padded), padded: padded)
                            let actual = pixels(FileActionSymbols.image(action, progress: progress, padded: padded)!, padded: padded)
                            XCTAssertEqual(actual, expected, "\(name.rawValue) \(action) \(padded) \(progress)")
                        }
                    }
                }
            }
        }
    }
}
