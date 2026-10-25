import AppKit
import XCTest
@testable import NotchPrototype

final class WorkspaceMorphTests:XCTestCase {
    func testHiddenNotchPreservesItsDivergentDraftUntilTheOwnerAcceptsIt(){
        XCTAssertFalse(UnifiedDraftHandoff.shouldAdopt(local:"notch draft",base:"old",remote:"new IDE draft",selectionChanged:false,sendAccepted:false))
        XCTAssertTrue(UnifiedDraftHandoff.shouldAdopt(local:"notch draft",base:"old",remote:"notch draft",selectionChanged:false,sendAccepted:false))
        XCTAssertTrue(UnifiedDraftHandoff.shouldAdopt(local:"old",base:"old",remote:"new IDE draft",selectionChanged:false,sendAccepted:false))
        XCTAssertTrue(UnifiedDraftHandoff.shouldAdopt(local:"sent",base:"old",remote:"",selectionChanged:false,sendAccepted:true))
    }

    func testWorkspaceContourUsesTheOriginalNotchShouldersAndMatchingSegments(){
        let rect=CGRect(x:0,y:0,width:420,height:320)
        let source=WorkspaceMorph.outline(rect,notch:true,radius:14)
        XCTAssertEqual(source,NotchSizing.path(canvas:rect.size,width:rect.width,height:rect.height))
        var sourceKinds=[CGPathElementType](),targetKinds=[CGPathElementType]()
        source.applyWithBlock{sourceKinds.append($0.pointee.type)}
        WorkspaceMorph.outline(rect,notch:false,radius:10).applyWithBlock{targetKinds.append($0.pointee.type)}
        XCTAssertEqual(sourceKinds,targetKinds)
        XCTAssertEqual(WorkspaceMorph.outline(rect,notch:false,radius:10).boundingBoxOfPath,rect)
    }
    func testWorkspaceFillsTheMenuBarAndDockWorkingArea() {
        let screen=CGRect(x:0,y:0,width:800,height:600)
        let frame=WorkspaceMorph.destination(saved:nil,screen:screen)
        XCTAssertEqual(frame,screen)
    }
    func testDisconnectedDisplayRestoresOntoCurrentScreen() {
        let screen=CGRect(x:-1440,y:40,width:1440,height:860)
        let frame=WorkspaceMorph.destination(saved:CGRect(x:4000,y:2000,width:1600,height:1000),screen:screen)
        XCTAssertEqual(frame,screen)
    }
    func testUnifiedStateDecodesThroughTheExistingBridge()throws {
        let object:[String:Any]=["ownerToken":"chat:1","composerDraft":"unsent","agents":[["id":"claude","name":"Claude","available":true]],"selectedAgent":"claude","workspaceVoiceActive":true,"project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:chat","threadName":"Shared","history":[],"readOnly":false,"approval":true,"approvalThreadId":"code:background","approvalRequestId":"epoch:request","approvalSummary":"Run tests","activity":["scene":"permission","text":"Approval needed"],"activeTasks":[],"notice":"","attachments":[]]
        let state=try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
        XCTAssertEqual(state.ownerToken,"chat:1");XCTAssertEqual(state.composerDraft,"unsent")
        XCTAssertEqual(state.agents?.first?.id,"claude");XCTAssertEqual(state.selectedAgent,"claude")
        XCTAssertEqual(CompactApprovalRequest(state:state)?.thread,"code:background")
    }
}
