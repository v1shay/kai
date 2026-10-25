import AppKit
import XCTest
@testable import NotchPrototype

final class WorkspaceMorphTests:XCTestCase {
    func testWorkspaceFitsSmallDisplaysAndKeepsMargin() {
        let screen=CGRect(x:0,y:0,width:800,height:600)
        let frame=WorkspaceMorph.destination(saved:nil,screen:screen)
        XCTAssertTrue(screen.insetBy(dx:24,dy:24).contains(frame))
        XCTAssertEqual(frame.width,752);XCTAssertEqual(frame.height,552)
    }
    func testDisconnectedDisplayRestoresOntoCurrentScreen() {
        let screen=CGRect(x:-1440,y:40,width:1440,height:860)
        let frame=WorkspaceMorph.destination(saved:CGRect(x:4000,y:2000,width:1600,height:1000),screen:screen)
        XCTAssertTrue(screen.insetBy(dx:24,dy:24).contains(frame))
    }
    func testUnifiedStateDecodesThroughTheExistingBridge()throws {
        let object:[String:Any]=["ownerToken":"chat:1","composerDraft":"unsent","agents":[["id":"claude","name":"Claude","available":true]],"selectedAgent":"claude","workspaceVoiceActive":true,"project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:chat","threadName":"Shared","history":[],"readOnly":false,"approval":true,"approvalThreadId":"code:background","approvalRequestId":"epoch:request","approvalSummary":"Run tests","activity":["scene":"permission","text":"Approval needed"],"activeTasks":[],"notice":"","attachments":[]]
        let state=try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
        XCTAssertEqual(state.ownerToken,"chat:1");XCTAssertEqual(state.composerDraft,"unsent")
        XCTAssertEqual(state.agents?.first?.id,"claude");XCTAssertEqual(state.selectedAgent,"claude")
        XCTAssertEqual(CompactApprovalRequest(state:state)?.thread,"code:background")
    }
}
