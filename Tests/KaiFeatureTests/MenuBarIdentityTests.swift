import AppKit
import XCTest
@testable import NotchPrototype

final class MenuBarIdentityTests:XCTestCase {
    @MainActor private func select(_ value:String,in menu:NSMenu)throws{
        for (index,item) in menu.items.enumerated(){
            if item.representedObject as? String == value{menu.performActionForItem(at:index);return}
            if let child=item.submenu,child.items.contains(where:{$0.representedObject as? String == value}){try select(value,in:child);return}
        }
        XCTFail("Missing menu identity: \(value)")
    }
    @MainActor func testEveryProviderCanBePinnedAndRestored()throws{
        _=NSApplication.shared
        let suite="KaiMenuIdentity-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let picker=MenuBarIdentityMenu(defaults:defaults);XCTAssertEqual(picker.mark,.kai)
        for (id,_) in MenuBarIdentityMenu.providers{
            try select("provider:"+id,in:picker.menu)
            XCTAssertEqual(picker.mark,.provider(id))
            XCTAssertEqual(MenuBarIdentityMenu(defaults:defaults).mark,.provider(id))
            XCTAssertNotNil(MenuBarIdentityMenu.image(picker.mark))
            picker.update(agent:"gemini")
            XCTAssertEqual(picker.mark,.provider(id),"Pinned identity follows the user's choice")
        }
        try select("kai",in:picker.menu);XCTAssertEqual(picker.mark,.kai)
    }
    @MainActor func testCurrentAgentTracksNativeAliasesAndKeepsLastIdentityAcrossLaunches()throws{
        _=NSApplication.shared
        let suite="KaiMenuIdentity-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let picker=MenuBarIdentityMenu(defaults:defaults)
        try select("current",in:picker.menu)
        picker.update(agent:"cursor-agent");XCTAssertEqual(picker.mark,.provider("cursor"))
        picker.update(agent:"agy");XCTAssertEqual(picker.mark,.provider("antigravity"))
        let ninja=MenuBarNinja(id:"studio:ice",name:"Zane",style:"ice",host:"Studio")
        picker.update(agent:"codex",currentNinja:ninja,ninjas:[ninja]);XCTAssertEqual(picker.mark,MenuBarMark(ninja:ninja))
        let restored=MenuBarIdentityMenu(defaults:defaults);XCTAssertEqual(restored.mark,picker.mark)
        var changes=0;picker.onChange={_ in changes += 1}
        for _ in 0..<20{picker.update(agent:"codex",currentNinja:ninja,ninjas:[ninja])}
        XCTAssertEqual(changes,0,"Repeated owner snapshots must not redraw the menu-bar icon")
    }
    @MainActor func testPersonalNinjaUsesItsCharacterAndFollowsSavedStyleChanges()throws{
        _=NSApplication.shared
        let suite="KaiMenuIdentity-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let picker=MenuBarIdentityMenu(defaults:defaults)
        let ice=MenuBarNinja(id:"studio:one",name:"Assistant",style:"ice",host:"Studio")
        let fire=MenuBarNinja(id:"local:two",name:"Assistant",style:"fire",host:"This Mac")
        picker.update(agent:"codex",ninjas:[ice,fire]);try select("ninja:"+ice.id,in:picker.menu)
        XCTAssertEqual(picker.mark,MenuBarMark(ninja:ice));XCTAssertEqual(MenuBarIdentityMenu(defaults:defaults).mark,picker.mark)
        for style in ["fire","lightning","earth","ice","energy","water"]{
            let character=MenuBarNinja(id:ice.id,name:ice.name,style:style,host:ice.host)
            picker.update(agent:"grok",ninjas:[character,fire]);XCTAssertEqual(picker.mark,MenuBarMark(ninja:character))
            let image=try XCTUnwrap(MenuBarIdentityMenu.image(picker.mark));XCTAssertFalse(image.isTemplate)
        }
    }
    @MainActor func testBadPreferenceIsKaiAndProviderImagesDontMutateOtherSurfaces()throws{
        _=NSApplication.shared
        let suite="KaiMenuIdentity-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        defaults.set("provider:removed",forKey:"menuBar.identity")
        let picker=MenuBarIdentityMenu(defaults:defaults);XCTAssertEqual(picker.selection,"kai");XCTAssertEqual(picker.mark,.kai)
        let shared=try XCTUnwrap(ProviderIcons.image("codex"));let originalSize=shared.size,template=shared.isTemplate
        let menu=try XCTUnwrap(MenuBarIdentityMenu.image(.provider("codex")))
        XCTAssertFalse(menu.isTemplate);XCTAssertEqual(shared.size,originalSize);XCTAssertEqual(shared.isTemplate,template)
    }
}
