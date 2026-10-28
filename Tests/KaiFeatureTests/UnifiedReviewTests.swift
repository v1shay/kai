import AppKit
import XCTest
@testable import NotchPrototype

final class UnifiedReviewTests:XCTestCase {
    func testNativeReviewDecodesAllProviderOptionsAndSecretQuestions()throws {
        let source=#"{"owner":"code:session","token":"epoch:opaque","title":"Permission","detail":"exact command","choices":[{"id":"once","label":"Allow once"},{"id":"always","label":"Allow matching actions"}],"questions":[{"id":"secret","header":"Token","question":"Enter a value","options":[],"secret":true,"multiple":false,"required":true}]}"#
        let review=try JSONDecoder().decode(KaiNativeReview.self,from:Data(source.utf8))
        XCTAssertEqual(review.choices.map(\.id),["once","always"])
        XCTAssertTrue(review.questions[0].secret);XCTAssertTrue(review.questions[0].required)
    }
    @MainActor func testNativeApprovalHasAnAnchoredReviewControlAndKeepsItsCommandSelectable()throws {
        _=NSApplication.shared
        let source=#"{"owner":"code:session","token":"epoch:opaque","title":"Permission","detail":"cargo test","choices":[{"id":"accept","label":"Allow once"}],"questions":[]}"#
        let review=try JSONDecoder().decode(KaiNativeReview.self,from:Data(source.utf8))
        var request=CompactApprovalRequest(preview:true);request.review=review
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:370,height:112))
        view.show(request);view.isHidden=false;view.layoutSubtreeIfNeeded()
        let button=try XCTUnwrap(view.subviews.compactMap{$0 as? NSButton}.first{$0.accessibilityLabel()=="Review request"})
        var captured:KaiNativeReview?;view.onReview={captured=$0};button.performClick(nil)
        XCTAssertEqual(captured,review);XCTAssertTrue(button.title.isEmpty);XCTAssertFalse(button.isBordered);XCTAssertTrue(view.reviewExpanded);XCTAssertTrue(view.reviewPinned)
        XCTAssertTrue(view.bounds.contains(button.frame))
        XCTAssertTrue(view.subviews.compactMap{$0 as? NSScrollView}.contains{($0.documentView as? NSTextView)?.isSelectable == true})
    }
    @MainActor func testInlineProviderChoiceKeepsItsExactContextAndCannotBeSentTwiceOrAfterReplacement()throws {
        _=NSApplication.shared
        func request(_ token:String)throws->CompactApprovalRequest{var request=CompactApprovalRequest(preview:true);request.review=try JSONDecoder().decode(KaiNativeReview.self,from:Data("{\"owner\":\"code:background\",\"token\":\"\(token)\",\"title\":\"Permission\",\"detail\":\"exact command\",\"choices\":[{\"id\":\"once\",\"label\":\"Allow once\"}],\"questions\":[]}".utf8));return request}
        func descendants(_ view:NSView)->[NSView]{[view]+view.subviews.flatMap(descendants)}
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88));view.show(try request("opaque-1"));view.setReviewExpanded(true,pinned:true);view.frame.size.height=view.preferredHeight;view.layoutSubtreeIfNeeded()
        var sent=[[String:Any]]();view.onReviewAction={action,fields,_ in XCTAssertEqual(action,"native_answer");sent.append(fields)}
        let choice=try XCTUnwrap(descendants(view).compactMap{$0 as? NSButton}.first{$0.accessibilityLabel()=="Allow once"});XCTAssertTrue(choice.title.isEmpty);XCTAssertFalse(choice.isBordered)
        choice.performClick(nil);choice.performClick(nil);XCTAssertEqual(sent.count,1);XCTAssertEqual(sent[0]["choice"] as? String,"once");XCTAssertEqual(sent[0]["threadId"] as? String,"code:background");XCTAssertEqual(sent[0]["approvalRequestId"] as? String,"opaque-1");XCTAssertNotNil(view.pendingID);XCTAssertFalse(view.reviewExpanded)
        view.reset();view.show(try request("opaque-2"));view.setReviewExpanded(true,pinned:true);choice.performClick(nil);XCTAssertEqual(sent.count,1,"Retired form controls must not target the replacement request")
        view.reset()
    }
    @MainActor func testHoverReviewExpandsInPlaceAndClickPinsUntilExplicitCollapse()throws {
        _=NSApplication.shared
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88));view.show(CompactApprovalRequest(preview:true));view.isHidden=false;view.layoutSubtreeIfNeeded()
        let icon=try XCTUnwrap(view.subviews.compactMap{$0 as? NSButton}.first{$0.accessibilityLabel()=="Review request"})
        let event=try XCTUnwrap(NSEvent.mouseEvent(with:.mouseMoved,location:.zero,modifierFlags:[],timestamp:0,windowNumber:0,context:nil,eventNumber:0,clickCount:0,pressure:0))
        icon.mouseEntered(with:event);XCTAssertTrue(view.reviewExpanded);XCTAssertFalse(view.reviewPinned)
        icon.performClick(nil);XCTAssertTrue(view.reviewPinned);icon.mouseExited(with:event);RunLoop.main.run(until:Date().addingTimeInterval(0.3));XCTAssertTrue(view.reviewExpanded)
        icon.performClick(nil);XCTAssertFalse(view.reviewExpanded);XCTAssertEqual(view.preferredHeight,88)
        view.reset()
    }
    @MainActor func testLargeReviewIsBoundedAndRetainsAllTextInItsScrollView()throws {
        _=NSApplication.shared
        var request=CompactApprovalRequest(preview:true);let text=Array(repeating:"Detailed provider context stays available.",count:90).joined(separator:"\n")
        request.review=KaiNativeReview(owner:"code:background",token:"bounded",title:"Review request",detail:text,choices:[],questions:[],kind:nil)
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88));view.maximumReviewHeight=160;view.show(request);view.setReviewExpanded(true,pinned:true);view.frame.size.height=view.preferredHeight;view.layoutSubtreeIfNeeded()
        XCTAssertEqual(view.preferredHeight,204)
        func descendants(_ view:NSView)->[NSView]{[view]+view.subviews.flatMap(descendants)}
        XCTAssertTrue(descendants(view).compactMap{$0 as? NSTextField}.contains{$0.stringValue==text && $0.isSelectable})
        XCTAssertTrue(descendants(view).compactMap{$0 as? NSScrollView}.contains{($0.documentView?.frame.height ?? 0)>$0.contentSize.height})
        view.reset()
    }
    @MainActor func testClosingReviewAndReopeningSamePendingRequestRestoresSelectableCommandAndSVGDecisions()throws {
        _=NSApplication.shared
        var request=CompactApprovalRequest(preview:true)
        request.review=KaiNativeReview(owner:"code:owner",token:"pending",title:"Approve command",detail:"exact command",choices:[KaiReviewOption(id:"once",label:"Allow once"),KaiReviewOption(id:"always",label:"Allow matching command in future")],questions:[],kind:nil)
        let view=CompactApprovalView(frame:CGRect(x:0,y:0,width:338,height:88));view.show(request);view.setReviewExpanded(true,pinned:true)
        RunLoop.main.run(until:Date().addingTimeInterval(0.3));view.reset();view.show(request);view.isHidden=false;view.layoutSubtreeIfNeeded()
        let command=try XCTUnwrap(view.subviews.compactMap{$0 as? NSScrollView}.first{($0.documentView as? NSTextView) != nil})
        XCTAssertFalse(command.isHidden);XCTAssertEqual(command.alphaValue,1);XCTAssertFalse(view.reviewExpanded)
        let approve=try XCTUnwrap(view.subviews.compactMap{$0 as? NSButton}.first{$0.accessibilityLabel()=="Approve"})
        var decisions=[String]();view.onDecision={action,owner in XCTAssertEqual(owner.id,request.id);decisions.append(action)};approve.performClick(nil)
        XCTAssertEqual(decisions,["approve"])
        view.setReviewExpanded(true,pinned:true)
        func descendants(_ view:NSView)->[NSView]{[view]+view.subviews.flatMap(descendants)}
        let choices=descendants(view).compactMap{$0 as? ApprovalButton}.filter{["Allow once","Allow matching command in future"].contains($0.accessibilityLabel() ?? "")}
        XCTAssertEqual(choices.count,2);XCTAssertTrue(choices.allSatisfy{$0.title.isEmpty && !$0.isBordered});view.reset()
    }

}
