import AppKit
import XCTest
@testable import NotchPrototype

final class ConversationViewportTests:XCTestCase {
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
