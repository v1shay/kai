import AppKit
import XCTest
@testable import NotchPrototype

final class CompactApprovalTests:XCTestCase {
    private func state(id:String="request",thread:String="background",summary:String="swift test")throws->KaiState{
        let object:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadId":"viewed-chat","threadName":"Chat","history":[],"readOnly":false,"approval":true,"approvalRequestId":id,"approvalThreadId":thread,"approvalSummary":summary,"activity":["scene":"permission","text":"Approval needed"],"activeTasks":[],"notice":"","attachments":[]]
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
    }
    func testExactCommandAndBackgroundRequestIdentityArePreserved()throws{
        let command="printf '%s\\n' \"a b\"\nswift test"
        let data=try JSONSerialization.data(withJSONObject:["command":command,"cwd":"/tmp/project","reason":"Run checks"])
        let initial=try state(summary:String(decoding:data,as:UTF8.self)),request=try XCTUnwrap(CompactApprovalRequest(state:initial))
        XCTAssertEqual(request.command,command);XCTAssertTrue(request.matches(initial))
        XCTAssertFalse(request.matches(try state(id:"new")));XCTAssertFalse(request.matches(try state(thread:"other")))
        let array=try XCTUnwrap(CompactApprovalRequest(state:state(summary:#"{"command":["echo","a b","$HOME"]}"#)))
        XCTAssertEqual(try JSONSerialization.jsonObject(with:Data(array.command.utf8)) as? [String],["echo","a b","$HOME"])
        let permissions=#"{"permissions":{"network":true},"reason":"Connect to service"}"#
        XCTAssertEqual(try CompactApprovalRequest(state:state(summary:permissions))?.command,permissions)
    }
    @MainActor func testThreeNativeControlsBindTheRequestAndBlockDuplicateDecisions()throws{
        _=NSApplication.shared
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88)),request=try XCTUnwrap(CompactApprovalRequest(state:state()))
        view.show(request);view.isHidden=false;view.layoutSubtreeIfNeeded()
        let buttons=view.subviews.compactMap{$0 as? NSButton}
        XCTAssertTrue(buttons.allSatisfy{!$0.isFlipped},"SVG scene coordinates use an upward Y axis")
        XCTAssertEqual(buttons.map{$0.accessibilityLabel()},["Approve","Don't approve","Stop chat"])
        var decisions=[String]();view.onDecision={action,selected in XCTAssertEqual(selected,request);decisions.append(action);view.begin("pending")}
        for button in buttons{
            button.performClick(nil);let count=decisions.count
            button.performClick(nil);XCTAssertEqual(decisions.count,count)
            view.finish("stale",error:nil);XCTAssertFalse(button.isEnabled)
            view.finish("pending",error:"Try again");XCTAssertTrue(button.isEnabled)
        }
        XCTAssertEqual(decisions,["approve","reject","approval_stop"])
        view.reset();XCTAssertTrue(view.isHidden);XCTAssertNil(view.request);XCTAssertNil(view.pendingID)
    }
    @MainActor func testLongCommandsScrollWithoutChangingTheCompactFrame()throws{
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88))
        let text=Array(repeating:"swift test --filter CompactApprovalTests",count:40).joined(separator:"\n")
        view.show(try XCTUnwrap(CompactApprovalRequest(state:state(summary:text))));view.isHidden=false;view.layoutSubtreeIfNeeded()
        let scroll=try XCTUnwrap(view.subviews.compactMap{$0 as? NSScrollView}.first),command=try XCTUnwrap(scroll.documentView as? NSTextView)
        XCTAssertEqual(command.string,text);XCTAssertTrue(command.isSelectable);XCTAssertFalse(command.isEditable)
        XCTAssertGreaterThan(command.frame.height,scroll.contentSize.height)
        XCTAssertEqual(view.bounds.height,88);XCTAssertEqual(CompactApprovalRequest.height(notch:39),151)
        let buttons=view.subviews.compactMap{$0 as? NSButton}
        XCTAssertTrue(buttons.allSatisfy{view.bounds.contains($0.frame)})
        XCTAssertFalse(buttons[0].frame.intersects(buttons[1].frame))
    }
    @MainActor func testPreviewIsLocalAndGlyphAnimationsAreFinite()throws{
        _=NSApplication.shared
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88)),preview=CompactApprovalRequest(preview:true)
        view.show(preview);view.isHidden=false;view.layoutSubtreeIfNeeded()
        XCTAssertTrue(view.subviews.compactMap{$0 as? NSTextField}.allSatisfy{$0.isHidden && $0.stringValue.isEmpty})
        XCTAssertTrue(preview.preview);XCTAssertFalse(preview.matches(try state(id:"preview",thread:"preview")))
        RunLoop.main.run(until:Date().addingTimeInterval(0.3))
        func descendants(_ layer:CALayer)->[CALayer]{[layer]+(layer.sublayers ?? []).flatMap(descendants)}
        let layers=try descendants(XCTUnwrap(view.layer)),animations=layers.flatMap{layer in (layer.animationKeys() ?? []).compactMap{layer.animation(forKey:$0)}}
        XCTAssertTrue(animations.allSatisfy{$0.repeatCount == 0})
        if let destination=ProcessInfo.processInfo.environment["KAI_APPROVAL_RENDER"]{
            view.layer?.backgroundColor=NSColor.black.cgColor
            let bitmap=try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in:view.bounds))
            view.cacheDisplay(in:view.bounds,to:bitmap)
            try XCTUnwrap(bitmap.representation(using:.png,properties:[:])).write(to:URL(fileURLWithPath:destination))
        }
        view.reset()
        XCTAssertTrue(layers.allSatisfy{$0.animation(forKey:"sceneLoop") == nil})
    }
}
