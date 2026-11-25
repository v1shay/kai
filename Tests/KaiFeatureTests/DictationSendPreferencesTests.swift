import XCTest
@testable import NotchPrototype

final class DictationSendPreferencesTests: XCTestCase {
    func testDefaultsAndIndependentPersistentOverrides() {
        let suite="KaiAutoSendTests.\(UUID().uuidString)"
        let defaults=UserDefaults(suiteName:suite)!
        defer {defaults.removePersistentDomain(forName:suite)}
        let settings=DictationSendPreferences(defaults:defaults)
        XCTAssertTrue(settings.enabled(for:.option))
        XCTAssertFalse(settings.enabled(for:.command))
        XCTAssertFalse(settings.enabled(for:.function))
        settings.set(false,for:.option)
        settings.set(true,for:.command)
        let restored=DictationSendPreferences(defaults:UserDefaults(suiteName:suite)!)
        XCTAssertFalse(restored.enabled(for:.option))
        XCTAssertTrue(restored.enabled(for:.command))
        XCTAssertFalse(restored.enabled(for:.function))
    }
}
