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
    func testInterruptedContourCoordinateConversionPreservesEveryCurve(){
        let path=WorkspaceMorph.outline(CGRect(x:34,y:70,width:680,height:350),notch:true,radius:14)
        let shifted=WorkspaceMorph.convert(path){CGPoint(x:$0.x+300,y:$0.y-800)}
        let restored=WorkspaceMorph.convert(shifted){CGPoint(x:$0.x-300,y:$0.y+800)}
        func points(_ path:CGPath)->[CGPoint]{var values=[CGPoint]();path.applyWithBlock{item in let e=item.pointee;let count=e.type == .addCurveToPoint ? 3:e.type == .addQuadCurveToPoint ? 2:e.type == .closeSubpath ? 0:1;values.append(contentsOf:(0..<count).map{e.points[$0]})};return values}
        let expected=points(path),actual=points(restored);XCTAssertEqual(expected.count,actual.count)
        for (a,b) in zip(expected,actual){XCTAssertEqual(a.x,b.x,accuracy:0.0000001);XCTAssertEqual(a.y,b.y,accuracy:0.0000001)}
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

extension WorkspaceMorphTests{
    func testWindowedMorphCanReachThePhysicalNotchAboveTheMenuBar(){
        XCTAssertGreaterThan(WorkspaceMorph.transitionLevel(.normal).rawValue,NSWindow.Level.statusBar.rawValue)
        let custom=NSWindow.Level(rawValue:NSWindow.Level.statusBar.rawValue+5)
        XCTAssertEqual(WorkspaceMorph.transitionLevel(custom),custom)
    }
}
