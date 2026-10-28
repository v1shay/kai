import AppKit
import XCTest
@testable import NotchPrototype

final class CompactApprovalTests:XCTestCase {
    @MainActor func testExpandedReviewHasOneActionRowAndPreservesProviderChoices()throws{
        _=NSApplication.shared
        var object=try JSONSerialization.jsonObject(with:approvalFixture()) as! [String:Any]
        object["nativeReview"]=["owner":"background","token":"request","title":"Approve command","detail":"Run a harmless command?","kind":"approval","questions":[],"choices":[["id":"accept","label":"Allow once"],["id":"cancel","label":"Stop turn"],["id":"acceptForSession","label":"Allow for this session"]]] as [String:Any]
        let request=try XCTUnwrap(CompactApprovalRequest(state:JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))))
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:200));view.show(request);view.isHidden=false
        func visible()->[NSButton]{view.subviews.compactMap{$0 as? NSButton}.filter{!$0.isHidden}}
        view.setReviewExpanded(true,pinned:true);view.layoutSubtreeIfNeeded()
        XCTAssertEqual(visible().compactMap{$0.accessibilityLabel()},["Review request","Allow once","Stop turn","Allow for this session"])
        func nestedButtons(_ root:NSView)->[NSButton]{root.subviews.flatMap{($0 as? NSButton).map{[$0]} ?? nestedButtons($0)}}
        let bodyButtons=view.subviews.filter{!($0 is NSButton)}.flatMap(nestedButtons)
        XCTAssertTrue(bodyButtons.isEmpty,"The review body duplicated the footer's action controls")
        var sent:[String:Any]?
        view.onReviewAction={action,fields,selected in XCTAssertEqual(action,"native_answer");XCTAssertEqual(selected.id,"request");sent=fields}
        visible().first{$0.accessibilityLabel()=="Allow for this session"}!.performClick(nil)
        XCTAssertEqual(sent?["choice"] as? String,"acceptForSession")
        XCTAssertEqual(sent?["threadId"] as? String,"background");XCTAssertEqual(sent?["approvalRequestId"] as? String,"request")
        XCTAssertFalse(view.reviewExpanded);XCTAssertEqual(visible().count,4)
        view.finish(sent?["requestId"] as? String,error:"Retry")
        view.setReviewExpanded(true,pinned:true);view.setReviewExpanded(false)
        XCTAssertEqual(visible().compactMap{$0.accessibilityLabel()},["Approve","Don't approve","Stop chat","Review request"])
        view.reset()
    }
    private func approvalFixture()throws->Data{
        try JSONSerialization.data(withJSONObject:["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadId":"viewed-chat","threadName":"Chat","history":[],"readOnly":false,"approval":true,"approvalRequestId":"request","approvalThreadId":"background","approvalSummary":"swift test","activity":["scene":"permission","text":"Approval needed"],"activeTasks":[],"notice":"","attachments":[]] as [String:Any])
    }
    @MainActor func testComposerTextRectCentersEmptyAndTypedLines()throws{
        let cell=ComposerTextCell(textCell:"");cell.font=NSFont.systemFont(ofSize:10);cell.isBordered=false;cell.isBezeled=false
        let bounds=CGRect(x:0,y:0,width:220,height:24)
        for text in ["","hello","Type, or hold"]{cell.stringValue=text;let rect=cell.drawingRect(forBounds:bounds);XCTAssertEqual(rect.midY,bounds.midY,accuracy:0.5);XCTAssertLessThan(rect.height,18);XCTAssertGreaterThan(rect.height,10)}
    }
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
    @MainActor func testFourNativeGlyphsBindTheRequestAndBlockDuplicateDecisions()throws{
        _=NSApplication.shared
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88)),request=try XCTUnwrap(CompactApprovalRequest(state:state()))
        view.show(request);view.isHidden=false;view.layoutSubtreeIfNeeded()
        let buttons=view.subviews.compactMap{$0 as? NSButton}
        XCTAssertTrue(buttons.allSatisfy{!$0.isFlipped},"SVG scene coordinates use an upward Y axis")
        XCTAssertEqual(buttons.map{$0.accessibilityLabel()},["Approve","Don't approve","Stop chat","Review request"])
        var decisions=[String]();view.onDecision={action,selected in XCTAssertEqual(selected,request);decisions.append(action);view.begin("pending")}
        for button in buttons where button.accessibilityLabel() != "Review request"{
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
    @MainActor func testToolFormsUseAnswersInsteadOfApprovalDecisions()throws{
        var object:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadId":"viewed-chat","threadName":"Chat","history":[],"readOnly":false,"approval":true,"approvalRequestId":"request","approvalThreadId":"background","approvalSummary":"Allow Gmail to update the draft?","activity":["scene":"permission","text":"Approval needed"],"activeTasks":[],"notice":"","attachments":[]]
        object["nativeReview"]=["owner":"background","token":"request","title":"Connected tool needs input","detail":"Allow Gmail to update the draft?","kind":"questions","choices":[],"questions":[["id":"confirm","header":"Confirm","question":"Allow Gmail to update the draft?","secret":false,"multiple":false,"required":true,"options":[["id":"true","label":"Yes"],["id":"false","label":"No"]]]]] as [String:Any]
        func request()throws->CompactApprovalRequest{try XCTUnwrap(CompactApprovalRequest(state:JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))))}
        let boolean=try request()
        XCTAssertEqual(boolean.response("approve")?.0,"native_answer")
        XCTAssertEqual(boolean.response("approve")?.1["answers"] as? [String:[String]],["confirm":["true"]])
        XCTAssertEqual(boolean.response("reject")?.1["answers"] as? [String:[String]],["confirm":["false"]])
        var review=object["nativeReview"] as! [String:Any];review["questions"]=[];object["nativeReview"]=review
        let form=try request();XCTAssertTrue(form.isQuestion);XCTAssertNil(form.response("approve"));XCTAssertEqual(form.response("reject")?.0,"native_cancel")
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88));view.show(form)
        var opened=false,decided=false;view.onReview={_ in opened=true};view.onDecision={_,_ in decided=true}
        view.subviews.compactMap{$0 as? NSButton}.first{$0.accessibilityLabel()=="Answer question"}!.performClick(nil)
        XCTAssertTrue(opened);XCTAssertFalse(decided)
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
