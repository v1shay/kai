import XCTest
@testable import NotchPrototype

final class ShortcutPreferencesTests: XCTestCase {
    private func withPreferences(_ body: (ShortcutPreferences) -> Void) {
        let suite="KaiShortcutTests.\(UUID().uuidString)"
        let defaults=UserDefaults(suiteName:suite)!
        defer {defaults.removePersistentDomain(forName:suite)}
        body(ShortcutPreferences(defaults:defaults))
    }

    func testDefaultsAndLegacyDictationMigrationPreserveDestinations() {
        withPreferences { settings in
            for trigger in DictationTrigger.allCases {
                XCTAssertEqual(settings.action(for:trigger),ShortcutAction.defaultAction(for:trigger))
                settings.defaults.set("dictation",forKey:"shortcutAction.\(trigger.rawValue)")
                XCTAssertEqual(settings.action(for:trigger),ShortcutAction.defaultAction(for:trigger))
                settings.defaults.set("removed-action",forKey:"shortcutAction.\(trigger.rawValue)")
                XCTAssertEqual(settings.action(for:trigger),ShortcutAction.defaultAction(for:trigger))
            }
            XCTAssertEqual(settings.action(for:.command),.sendActiveChat)
            XCTAssertEqual(settings.action(for:.function),.sendNewChat)
            XCTAssertEqual(settings.action(for:.option),.sendNewChatWithAppshot)
        }
    }

    func testEveryActionCanBeAssignedToEveryKeyWithoutChangingAutoSend() {
        withPreferences { settings in
            for trigger in DictationTrigger.allCases {
                for action in ShortcutAction.allCases {
                    let other=DictationTrigger.allCases.first{$0 != trigger}!
                    let unchanged=settings.action(for:other)
                    settings.set(action,for:trigger)
                    XCTAssertEqual(ShortcutPreferences(defaults:settings.defaults).action(for:trigger),action)
                    XCTAssertEqual(settings.action(for:other),unchanged)
                }
            }
            let dictation=DictationSendPreferences(defaults:settings.defaults)
            XCTAssertTrue(dictation.enabled(for:.option))
            XCTAssertFalse(dictation.enabled(for:.command))
        }
    }

    func testHoldTimingPreservesFastOptionDictationAndDeliberateVoice() {
        XCTAssertEqual(ShortcutAction.sendNewChatWithAppshot.holdDuration(for:.option),0.2)
        for trigger in DictationTrigger.allCases {
            for action in ShortcutAction.allCases where action.isVoice {
                XCTAssertEqual(action.holdDuration(for:trigger),0)
            }
        }
        XCTAssertEqual(ShortcutAction.sendNewChat.holdDuration(for:.function),1)
    }

    func testModifierChordsPreserveKeyboardShortcutsAndControlOption() {
        for mask in 0..<32 {
            let command=mask & 1 != 0,option=mask & 2 != 0,control=mask & 4 != 0,function=mask & 8 != 0,shift=mask & 16 != 0
            let expected:DictationTrigger? = mask == 1 ? .command : mask == 2 ? .option : mask == 8 ? .function : nil
            XCTAssertEqual(ShortcutPreferences.heldTrigger(command:command,option:option,control:control,function:function,shift:shift),expected,"modifier mask \(mask)")
        }
    }
    func testChordReleaseCannotLaunchAnAppshotOrVoiceShortcut() {
        var gesture=ShortcutGesture()
        XCTAssertEqual(gesture.update(command:false,option:true,control:false,function:false,shift:false),.begin(.option))
        XCTAssertEqual(gesture.update(command:false,option:true,control:true,function:false,shift:false),.cancel)
        XCTAssertEqual(gesture.update(command:false,option:true,control:false,function:false,shift:false),.cancel)
        XCTAssertEqual(gesture.update(command:false,option:false,control:false,function:false,shift:false),.cancel)
        XCTAssertEqual(gesture.update(command:false,option:true,control:false,function:false,shift:false),.begin(.option))
    }
    func testHoldingDoesNotRepeatAndShiftCancelsUntilKeyIsReleased() {
        var gesture=ShortcutGesture()
        XCTAssertEqual(gesture.update(command:true,option:false,control:false,function:false,shift:false),.begin(.command))
        XCTAssertEqual(gesture.update(command:true,option:false,control:false,function:false,shift:false),.unchanged)
        XCTAssertEqual(gesture.update(command:true,option:false,control:false,function:false,shift:true),.cancel)
        XCTAssertEqual(gesture.update(command:true,option:false,control:false,function:false,shift:false),.cancel)
        _=gesture.update(command:false,option:false,control:false,function:false,shift:false)
        XCTAssertEqual(gesture.update(command:true,option:false,control:false,function:false,shift:false),.begin(.command))
    }
}
