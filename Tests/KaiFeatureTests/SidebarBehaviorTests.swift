import AppKit
import XCTest
@testable import NotchPrototype

final class SidebarBehaviorTests:XCTestCase {
    @MainActor func testRowPaddingOpensTheNearestChatWithoutHidingIt()throws {
        let suite="KaiSidebarPadding-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let ui=MiniAppInterface(defaults:defaults)
        ui.layout(in:CGRect(x:0,y:0,width:450,height:400));ui.setState(try state())
        let left=try XCTUnwrap(ui.layer.sublayers?.first)
        let row=try XCTUnwrap(left.sublayers?.first{$0.name == "Running"})
        var opened=[String]();ui.onChat={opened.append($0)}
        XCTAssertTrue(ui.handleClick(CGPoint(x:left.frame.minX+row.frame.minX+18,y:left.frame.minY+row.frame.maxY+1)))
        XCTAssertEqual(opened,["running"])
        XCTAssertTrue(left.sublayers?.contains{$0.name == "Running"} ?? false)
    }
    @MainActor func testLoadingHighlightTracksChatThroughScrollingAndRowVirtualization()throws {
        let suite="KaiSidebarLoading-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let ui=MiniAppInterface(defaults:defaults)
        ui.layout(in:CGRect(x:0,y:0,width:450,height:240))
        ui.setState(try state(chats:(0..<80).map{["id":"chat-\($0)","name":"Chat \($0)","status":"idle"]}))
        let left=try XCTUnwrap(ui.layer.sublayers?.first)
        let originalRow=try XCTUnwrap(left.sublayers?.first{$0.name == "Chat 0"})
        var opened=[String]();ui.onChat={opened.append($0)}
        _=ui.handleClick(CGPoint(x:left.frame.minX+originalRow.frame.minX+18,y:left.frame.minY+originalRow.frame.midY))
        XCTAssertEqual(opened,["chat-0"])
        let request=try XCTUnwrap(ui.pendingRequestID)
        let highlight=try XCTUnwrap(left.sublayers?.first{$0.name == "loadingHighlight"})
        XCTAssertEqual(highlight.frame,originalRow.frame.insetBy(dx:-2,dy:-2))
        let originalFrame=highlight.frame
        let point=CGPoint(x:20,y:100)
        ui.scroll(-32,at:point)
        XCTAssertEqual(highlight.frame.minY,originalFrame.minY+32)
        ui.scroll(-100000,at:point)
        XCTAssertNil(left.sublayers?.first{$0.name == "Chat 0"})
        XCTAssertTrue(highlight.isHidden)
        ui.scroll(100000,at:point)
        let recreated=try XCTUnwrap(left.sublayers?.first{$0.name == "Chat 0"})
        XCTAssertFalse(recreated === originalRow)
        XCTAssertFalse(highlight.isHidden)
        XCTAssertEqual(highlight.frame,recreated.frame.insetBy(dx:-2,dy:-2))
        ui.layout(in:CGRect(x:0,y:0,width:450,height:400))
        XCTAssertEqual(highlight.frame,recreated.frame.insetBy(dx:-2,dy:-2))
        XCTAssertTrue(highlight.superlayer === left);XCTAssertTrue(left.masksToBounds)
        XCTAssertEqual(ui.pendingRequestID,request)
        ui.finishPending(requestID:request)
        XCTAssertNil(ui.pendingRequestID);XCTAssertEqual(highlight.opacity,0)
    }
    private func state(chats:[[String:String]]?=nil)throws->KaiState {
        let object:[String:Any] = ["project":"/one","projects":[["path":"/one","name":"One"],["path":"/two","name":"Two"]],"chats":chats ?? [["id":"running","name":"Running","status":"active"],["id":"idle","name":"Idle","status":"idle"]],"standaloneChats":[["id":"solo","name":"Solo","status":"idle"]],"standalone":false,"threadId":"idle","threadName":"Idle","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[["id":"running","name":"Running","scene":"thinking","text":"Working"]],"notice":"","attachments":[]]
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
    }
    @MainActor func testExpandedThemeFollowsViewedChatWhileIndicatorChanges(){
        let assignments=["viewed":"blue","other":"orange"]
        for indicator in ["orange","green","purple"] {
            XCTAssertEqual(ConversationTheme.petID(expanded:true,viewedThread:"viewed",taskPets:assignments,indicatorPet:indicator,fallback:"default"),"blue")
            XCTAssertEqual(ConversationTheme.petID(expanded:false,viewedThread:"viewed",taskPets:assignments,indicatorPet:indicator,fallback:"default"),indicator)
        }
        XCTAssertEqual(ConversationTheme.petID(expanded:true,viewedThread:"other",taskPets:assignments,indicatorPet:"green",fallback:"default"),"orange")
    }
    @MainActor func testFinishedChatHeaderDoesNotShowAnotherChatsProgress()throws {
        let state=try state(),ui=MiniAppInterface()
        ui.layout(in:CGRect(x:0,y:0,width:450,height:400));ui.setState(state)
        func text(in layer:CALayer)->[String] {
            let own=(layer as? CATextLayer).flatMap{($0.string as? NSAttributedString)?.string}.map{[$0]} ?? []
            return own+(layer.sublayers ?? []).flatMap{text(in:$0)}
        }
        XCTAssertTrue(text(in:ui.layer).contains("Idle"))
        XCTAssertFalse(text(in:ui.layer).contains(where:{$0.contains("Working")}))
        XCTAssertEqual(state.viewedActivity.scene,"off")
    }
    @MainActor func testActiveAppearsFirstAndHideClicksPersistWithoutOpeningChats()throws {
        let suite="KaiSidebarTests-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let state=try state(),ui=MiniAppInterface(defaults:defaults)
        ui.layout(in:CGRect(x:0,y:0,width:450,height:400));ui.setState(state)
        let left=try XCTUnwrap(ui.layer.sublayers?.first)
        let titles=left.sublayers?.compactMap{$0 as? CATextLayer} ?? []
        let active=try XCTUnwrap(titles.first{$0.string as? String == "ACTIVE"})
        let chats=try XCTUnwrap(titles.first{$0.string as? String == "CHATS"})
        XCTAssertGreaterThan(active.frame.minY,chats.frame.minY)
        XCTAssertEqual(left.sublayers?.filter{$0.name == "Running"}.count,1)
        var opened=0;ui.onChat={_ in opened+=1};ui.onProject={_ in opened+=1}
        let running=try XCTUnwrap(left.sublayers?.first{$0.name == "Running"})
        XCTAssertTrue(ui.handleClick(CGPoint(x:running.frame.maxX-4,y:left.frame.minY+running.frame.midY)))
        XCTAssertEqual(opened,0)
        XCTAssertFalse(left.sublayers?.contains{$0.name == "Running"} ?? true)
        let project=try XCTUnwrap(left.sublayers?.first{$0.name == "/one"})
        XCTAssertTrue(ui.handleClick(CGPoint(x:project.frame.maxX-4,y:left.frame.minY+project.frame.midY)))
        XCTAssertEqual(opened,0)
        XCTAssertFalse(left.sublayers?.contains{$0.name == "/one" || $0.name == "Idle"} ?? true)
        let restored=MiniAppInterface(defaults:defaults)
        restored.layout(in:CGRect(x:0,y:0,width:450,height:400));restored.setState(state)
        let restoredLeft=try XCTUnwrap(restored.layer.sublayers?.first)
        XCTAssertFalse(restoredLeft.sublayers?.contains{$0.name == "Running" || $0.name == "/one" || $0.name == "Idle"} ?? true)
        XCTAssertTrue(restoredLeft.sublayers?.contains{$0.name == "/two"} ?? false)
        XCTAssertTrue(restoredLeft.sublayers?.contains{$0.name == "Solo"} ?? false)
    }
}
