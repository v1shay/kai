import AppKit
import XCTest
@testable import NotchPrototype

final class UnifiedVoicePreferenceTests:XCTestCase {
    @MainActor func testOwnerVoiceUpdatesBothMenusWithoutSendingAnotherCommand()throws {
        let name="KaiUnifiedVoice-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:name))
        defer{defaults.removePersistentDomain(forName:name)}
        let menu=RealtimeVoiceMenu(defaults:defaults);var changes=0
        menu.onSelection={_ in changes += 1}
        menu.synchronize("maple");XCTAssertEqual(menu.selection,.maple)
        menu.synchronize("default");XCTAssertEqual(menu.selection,.codexDefault)
        menu.synchronize("not-a-voice");XCTAssertEqual(menu.selection,.codexDefault)
        XCTAssertEqual(changes,0)
    }
}
