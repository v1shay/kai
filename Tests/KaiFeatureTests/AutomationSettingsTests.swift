import AppKit
import XCTest
@testable import NotchPrototype

final class AutomationSettingsTests:XCTestCase {
    @MainActor func testDefaultModelMenuPersistsAndRoutesOnlyNewChats()throws {
        _=NSApplication.shared
        let suite="KaiAutomationTests-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let menu=DefaultModelMenu(defaults:defaults),preferences=DefaultModelPreferences(defaults:defaults)
        let model=KaiModel(model:"economy",displayName:"Economy",supportedReasoningEfforts:[])
        menu.update([model])
        let index=try XCTUnwrap(menu.menu.items.firstIndex{$0.representedObject as? String == "economy"})
        menu.menu.performActionForItem(at:index)
        XCTAssertEqual(preferences.selectedID,"economy")
        for action in ["new","new_project"]{
            XCTAssertEqual(preferences.fields(action:action)["defaultModel"] as? String,"economy")
        }
        XCTAssertTrue(preferences.fields(action:"voice_start").isEmpty)
        XCTAssertTrue(preferences.fields(action:"send").isEmpty)
        XCTAssertTrue(preferences.fields(action:"settings").isEmpty)
        let restored=DefaultModelMenu(defaults:defaults);restored.update([model])
        XCTAssertEqual(restored.menu.items.filter{$0.state == .on}.map{$0.representedObject as? String},["economy"])
        restored.update([])
        XCTAssertTrue(restored.menu.items.contains{$0.state == .on && !$0.isEnabled && $0.title.contains("unavailable")})
        restored.menu.performActionForItem(at:0)
        XCTAssertNil(preferences.selectedID)
        XCTAssertFalse(restored.menu.items.contains{$0.title.contains("unavailable")})
    }
    @MainActor func testTimingSlidersPersistIndependentlyAndRestoreOriginalDefaults()throws {
        _=NSApplication.shared
        let suite="KaiTimingTests-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let preferences=ShortcutPreferences(defaults:defaults),menu=ShortcutTimingMenu(defaults:defaults)
        XCTAssertEqual(preferences.holdDuration(for:.command),1)
        XCTAssertEqual(preferences.holdDuration(for:.function),1)
        XCTAssertEqual(preferences.holdDuration(for:.option),0.2)
        let slider=try XCTUnwrap(menu.menu.items.first?.view?.subviews.compactMap{$0 as? NSSlider}.first)
        slider.doubleValue=2.4;_=slider.sendAction(slider.action,to:slider.target)
        XCTAssertEqual(preferences.holdDuration(for:.command),2.4)
        XCTAssertEqual(preferences.holdDuration(for:.option),0.2)
        preferences.set(.realtimeVoice,for:.function)
        XCTAssertEqual(preferences.holdDuration(for:.function),0.25)
        preferences.setHoldDuration(.nan,for:.function)
        XCTAssertEqual(preferences.holdDuration(for:.function),0.25)
        preferences.setHoldDuration(99,for:.option)
        XCTAssertEqual(preferences.holdDuration(for:.option),5)
        preferences.setHoldDuration(-1,for:.function)
        XCTAssertEqual(preferences.holdDuration(for:.function),0.1)
        menu.menu.performActionForItem(at:menu.menu.items.count-1)
        XCTAssertEqual(preferences.holdDuration(for:.command),1)
        XCTAssertEqual(preferences.holdDuration(for:.function),0.25)
        XCTAssertEqual(preferences.holdDuration(for:.option),0.2)
    }
    @MainActor func testMenuOrganizationPreservesEveryControlAndItsLiveObject() {
        let menu=NSMenu(),quick=menu.addItem(withTitle:"Open",action:nil,keyEquivalent:"")
        let appearance=menu.addItem(withTitle:"Glass",action:nil,keyEquivalent:"")
        let view=NSView();appearance.view=view
        let extra=menu.addItem(withTitle:"Future setting",action:nil,keyEquivalent:"")
        let quit=menu.addItem(withTitle:"Quit",action:nil,keyEquivalent:"")
        NotchMenuOrganizer.organize(menu,quick:[quick],sections:[.init(title:"Appearance",items:[appearance])],footer:[quit])
        XCTAssertTrue(menu.items.first === quick);XCTAssertTrue(menu.items.last === quit)
        XCTAssertTrue(menu.items.first{$0.title == "Appearance"}?.submenu?.items.first === appearance)
        XCTAssertTrue(appearance.view === view)
        XCTAssertTrue(menu.items.first{$0.title == "More controls"}?.submenu?.items.first === extra)
    }
}
