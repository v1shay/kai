import AppKit
import XCTest
@testable import NotchPrototype

final class RealtimeVoiceMenuTests:XCTestCase {
    @MainActor func testMenuActionsPersistSelectionAndCheckExactlyOneVoice()throws {
        _=NSApplication.shared
        let suite="KaiRealtimeVoiceTests-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        defaults.set("local-piper.onnx",forKey:"speechModel")
        let picker=RealtimeVoiceMenu(defaults:defaults)
        var changes=[RealtimeVoiceChoice]();picker.onSelection={changes.append($0)}
        XCTAssertEqual(picker.selection,.codexDefault)
        for choice in RealtimeVoiceChoice.allCases {
            let index=try XCTUnwrap(picker.menu.items.firstIndex{$0.representedObject as? String == choice.rawValue})
            picker.menu.performActionForItem(at:index)
            XCTAssertEqual(picker.selection,choice)
            XCTAssertEqual(RealtimeVoiceMenu(defaults:defaults).selection,choice)
            XCTAssertEqual(picker.menu.items.filter{$0.state == .on}.map{$0.title},[choice.title])
            XCTAssertEqual(changes.last,choice)
        }
        XCTAssertEqual(defaults.string(forKey:"speechModel"),"local-piper.onnx")
    }
    @MainActor func testUnknownSavedVoiceUsesCodexDefault()throws {
        let suite="KaiRealtimeVoiceTests-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        defaults.set("removed-voice",forKey:"realtimeVoice")
        let picker=RealtimeVoiceMenu(defaults:defaults)
        XCTAssertEqual(picker.selection,.codexDefault)
        XCTAssertNil(picker.selection.apiValue)
        XCTAssertEqual(picker.menu.items.filter{$0.state == .on}.map{$0.title},["Codex default"])
    }
}
