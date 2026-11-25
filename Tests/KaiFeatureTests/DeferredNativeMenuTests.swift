import AppKit
import XCTest
@testable import NotchPrototype

final class DeferredNativeMenuTests:XCTestCase {
    @MainActor func testBuildsLatestStateOnceAndPreservesShellActions(){
        let menu=NSMenu(),shell=menu.addItem(withTitle:"Existing action",action:nil,keyEquivalent:"")
        var revision="initial",builds=0
        let owner=DeferredNativeMenu(menu:menu){menu in
            builds+=1
            menu.addItem(withTitle:revision,action:nil,keyEquivalent:"")
        }
        XCTAssertEqual(builds,0);XCTAssertEqual(menu.items,[shell])
        revision="latest"
        owner.menuNeedsUpdate(menu);owner.menuNeedsUpdate(menu)
        XCTAssertEqual(builds,1);XCTAssertEqual(menu.items.map(\.title),["Existing action","latest"])
        XCTAssertTrue(menu.delegate === owner)
    }
    @MainActor func testReentrantUpdateDoesNotDuplicateControls(){
        let menu=NSMenu();var builds=0;var owner:DeferredNativeMenu!
        owner=DeferredNativeMenu(menu:menu){menu in
            builds+=1;owner.menuNeedsUpdate(menu)
            menu.addItem(withTitle:"One",action:nil,keyEquivalent:"")
        }
        owner.menuNeedsUpdate(menu)
        XCTAssertEqual(builds,1);XCTAssertEqual(menu.items.count,1)
        owner=nil
    }
}
