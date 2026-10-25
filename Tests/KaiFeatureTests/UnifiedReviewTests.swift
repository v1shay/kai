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
        let button=try XCTUnwrap(view.subviews.compactMap{$0 as? NSButton}.first{$0.title=="Review…"})
        var captured:KaiNativeReview?;view.onReview={captured=$0};button.performClick(nil)
        XCTAssertEqual(captured,review)
        XCTAssertTrue(view.bounds.contains(button.frame))
        XCTAssertTrue(view.subviews.compactMap{$0 as? NSScrollView}.contains{($0.documentView as? NSTextView)?.isSelectable == true})
    }
}
