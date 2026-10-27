import AppKit
import XCTest
@testable import NotchPrototype

final class ConversationViewportTests:XCTestCase {
    @MainActor func testShortNotchMessagesSitAboveComposerAcrossResizeAndGrowth(){
        _=NSApplication.shared
        let scroll=ConversationScrollView(frame:CGRect(x:0,y:0,width:300,height:200)),text=ConversationTextView()
        text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true
        text.textContainerInset=CGSize(width:3,height:24);scroll.documentView=text
        for size in [CGSize(width:300,height:200),CGSize(width:450,height:360),CGSize(width:280,height:180)] {
            scroll.setFrameSize(size);scroll.tile();text.string="First message\nFirst response"
            ConversationViewport.bottom(text,in:scroll)
            let used=text.layoutManager!.usedRect(for:text.textContainer!).maxY
            XCTAssertGreaterThan(text.leadingSpace,0)
            XCTAssertEqual(text.textContainerOrigin.y+ceil(used),scroll.contentSize.height-24,accuracy:1)
            XCTAssertEqual(scroll.contentView.bounds.minY,0,accuracy:1)
            text.string=String(repeating:"More conversation text\n",count:100)+"LAST LINE"
            ConversationViewport.bottom(text,in:scroll)
            XCTAssertEqual(text.leadingSpace,0)
            XCTAssertTrue(ConversationViewport.isAtBottom(text,in:scroll))
            XCTAssertEqual(scroll.contentView.bounds.maxY,text.frame.height,accuracy:1)
        }
    }
    @MainActor func testPagedHistoryCanReturnToLatestAtTheOldestBoundary(){
        _=NSApplication.shared
        let view=ConversationHistoryControl(frame:CGRect(x:0,y:0,width:280,height:24));view.update(hasMore:false,loading:false,earlier:true);view.layoutSubtreeIfNeeded()
        XCTAssertFalse(view.isHidden)
        let latest=view.subviews.compactMap{$0 as? NSButton}.first{$0.title == "Latest"}!
        XCTAssertFalse(latest.isHidden);XCTAssertEqual(latest.frame.midX,view.bounds.midX)
        var called=false;view.onLatest={called=true};latest.performClick(nil);XCTAssertTrue(called)
    }
    @MainActor func testFirstLayoutShowsFinalLineAtEverySizeAndCanReopenAtBottom(){
        for size in [CGSize(width:260,height:160),CGSize(width:500,height:350),CGSize(width:700,height:500)] {
            let scroll=ConversationScrollView(frame:CGRect(origin:.zero,size:size)),text=NSTextView(frame:CGRect(origin:.zero,size:size))
            text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true
            text.textContainerInset=CGSize(width:3,height:3);text.autoresizingMask=[.width]
            scroll.documentView=text
            text.string=String(repeating:"Response line with wrapped text and unicode 🌊\n",count:100)+"FINAL RESPONSE LINE"
            ConversationViewport.bottom(text,in:scroll)
            XCTAssertGreaterThan(text.frame.height,size.height)
            XCTAssertTrue(ConversationViewport.isAtBottom(text,in:scroll))
            XCTAssertEqual(scroll.contentView.bounds.maxY,text.frame.height,accuracy:1)
            scroll.contentView.scroll(to:.zero)
            XCTAssertFalse(ConversationViewport.isAtBottom(text,in:scroll))
            ConversationViewport.bottom(text,in:scroll)
            XCTAssertEqual(scroll.contentView.bounds.maxY,text.frame.height,accuracy:1)
        }
    }
    @MainActor func testShortResponseNeedsNoBlankScrollSpace(){
        let scroll=ConversationScrollView(frame:CGRect(x:0,y:0,width:300,height:200)),text=NSTextView()
        text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true;scroll.documentView=text;text.string="Short response"
        ConversationViewport.bottom(text,in:scroll)
        XCTAssertEqual(scroll.contentView.bounds.minY,0)
        XCTAssertEqual(text.frame.height,scroll.contentSize.height,accuracy:1)
    }
}
