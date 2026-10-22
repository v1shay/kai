import AppKit
import XCTest
@testable import NotchPrototype

final class ConversationRendererTests:XCTestCase {
    private func message(_ id:String,_ text:String)throws->KaiMessage {
        let data=try JSONSerialization.data(withJSONObject:["id":id,"kind":"assistant","lines":[["text":text,"style":""]]])
        return try JSONDecoder().decode(KaiMessage.self,from:data)
    }
    @MainActor func testLargeHistoryStreamingReplacesOnlyChangedMessageAndPreservesSelection()throws {
        let scroll=NSScrollView(frame:CGRect(x:0,y:0,width:350,height:250)),text=NSTextView(frame:CGRect(x:0,y:0,width:350,height:20000))
        scroll.documentView=text
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        var history=try (0..<2000).map{try message("\($0)","Message \($0) 🌊")}
        _=renderer.render(history,threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white])
        XCTAssertEqual(renderer.lastReplacedBlockCount,2000)
        text.setSelectedRange(NSRange(location:3,length:4))
        for revision in 0..<20 {
            history[1999]=try message("1999","Streaming revision \(revision)")
            _=renderer.render(history,threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white])
            XCTAssertEqual(renderer.lastReplacedBlockCount,1)
            XCTAssertEqual(text.selectedRange(),NSRange(location:3,length:4))
        }
        _=renderer.render(history,threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white])
        XCTAssertEqual(renderer.lastReplacedBlockCount,0)
    }
    @MainActor func testPrependingPreservesViewportSelectionAndExistingBlocks()throws {
        let scroll=ConversationScrollView(frame:CGRect(x:0,y:0,width:300,height:160)),text=NSTextView()
        text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true
        text.textContainerInset=CGSize(width:3,height:24);scroll.documentView=text
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        let recent=try [message("3",String(repeating:"Recent text 🌊\n",count:35)),message("4","Final response")]
        _=renderer.render(recent,threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white])
        ConversationViewport.layout(text,in:scroll)
        scroll.contentView.scroll(to:CGPoint(x:0,y:67))
        let anchor=try XCTUnwrap(ConversationViewport.anchor(text,in:scroll))
        text.setSelectedRange(NSRange(location:12,length:8))
        let older=try [message("1",String(repeating:"Earlier wrapped message with longer lines\n",count:30)),message("2","Earlier response")]
        _=renderer.render(older+recent,threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white],anchorCharacter:anchor.character)
        XCTAssertEqual(renderer.lastReplacedBlockCount,2)
        let added=renderer.prependedCharacterCount
        XCTAssertGreaterThan(added,0)
        XCTAssertEqual(renderer.mappedAnchorCharacter,anchor.character+added)
        XCTAssertEqual(text.selectedRange(),NSRange(location:12+added,length:8))
        ConversationViewport.layout(text,in:scroll)
        ConversationViewport.restore(anchor,adding:added,text:text,scroll:scroll)
        let restored=try XCTUnwrap(ConversationViewport.anchor(text,in:scroll))
        XCTAssertEqual(restored.character,anchor.character+added)
        XCTAssertEqual(restored.offset,anchor.offset,accuracy:0.5)
        // Later layout changes above the visible line must retain the same anchor.
        let taller=try [message("1",String(repeating:"More older lines\n",count:100)),older[1]]
        _=renderer.render(taller+recent,threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white],anchorCharacter:restored.character)
        ConversationViewport.layout(text,in:scroll)
        let shifted=(text.string as NSString).range(of:"Recent text").location
        XCTAssertEqual(renderer.mappedAnchorCharacter,anchor.character+shifted)
        ConversationViewport.restore(anchor,adding:shifted,text:text,scroll:scroll)
        XCTAssertEqual(ConversationViewport.anchor(text,in:scroll)?.character,anchor.character+shifted)
    }
    @MainActor func testActivityRowsAreDimAndCompletionOnlyReplacesThatRow()throws {
        let scroll=NSScrollView(),text=NSTextView();scroll.documentView=text
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        func activity(_ value:String)throws->KaiMessage {
            let object:[String:Any]=["id":"command","kind":"activity","lines":[["text":value,"style":"activity","mathEnabled":false]]]
            return try JSONDecoder().decode(KaiMessage.self,from:JSONSerialization.data(withJSONObject:object))
        }
        let before=try message("before","Checking the result"),after=try message("after","The result is ready")
        _=renderer.render([before,try activity("Running a command…"),after],threadID:"a",in:text,scroll:scroll,renderMath:true,palette:[.red])
        let range=(text.string as NSString).range(of:"Running a command")
        let storage=try XCTUnwrap(text.textStorage)
        let color=try XCTUnwrap(storage.attribute(.foregroundColor,at:range.location,effectiveRange:nil) as? NSColor)
        XCTAssertEqual(color.alphaComponent,0.42,accuracy:0.01)
        XCTAssertNil(storage.attribute(.kaiGradientPosition,at:range.location,effectiveRange:nil))
        let font=try XCTUnwrap(storage.attribute(.font,at:range.location,effectiveRange:nil) as? NSFont)
        XCTAssertTrue(NSFontManager.shared.traits(of:font).contains(.italicFontMask))
        text.setSelectedRange(NSRange(location:2,length:3))
        _=renderer.render([before,try activity("Ran a command"),after],threadID:"a",in:text,scroll:scroll,renderMath:true,palette:[.red])
        XCTAssertEqual(renderer.lastReplacedBlockCount,1)
        XCTAssertEqual(text.selectedRange(),NSRange(location:2,length:3))
        XCTAssertTrue(text.string.contains("Ran a command\nThe result is ready"))
        XCTAssertNotNil(storage.attribute(.attachment,at:(text.string as NSString).range(of:"Ran a command").location-2,effectiveRange:nil))
    }
    @MainActor func testActivitySpacingTrimsBlankSeparatorsAndUsesOneCachedIcon()throws {
        let text=NSTextView(),scroll=NSScrollView();scroll.documentView=text
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        let object:[String:Any]=["id":"tools","kind":"activity","activityIcon":"terminal","lines":[["text":"Ran commands, viewed images","style":"activity","mathEnabled":false]]]
        let activity=try JSONDecoder().decode(KaiMessage.self,from:JSONSerialization.data(withJSONObject:object))
        func spaced(_ id:String,_ lines:[String])throws->KaiMessage {
            let object:[String:Any]=["id":id,"kind":"assistant","lines":lines.map{["text":$0,"style":""]}]
            return try JSONDecoder().decode(KaiMessage.self,from:JSONSerialization.data(withJSONObject:object))
        }
        _=renderer.render([try spaced("before",["Before","",""]),activity,try spaced("after",["","After","",""])],threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white])
        XCTAssertEqual(text.string,"Before\n\u{fffc} Ran commands, viewed images\nAfter\n")
        let storage=try XCTUnwrap(text.textStorage)
        for label in ["Before","Ran commands","After"] {
            let range=(text.string as NSString).range(of:label)
            let paragraph=try XCTUnwrap(storage.attribute(.paragraphStyle,at:range.location,effectiveRange:nil) as? NSParagraphStyle)
            XCTAssertEqual(paragraph.paragraphSpacing,5)
            XCTAssertEqual(paragraph.paragraphSpacingBefore,0)
        }
        var attachments=0
        storage.enumerateAttribute(.attachment,in:NSRange(location:0,length:storage.length)){value,_,_ in if value != nil {attachments += 1}}
        XCTAssertEqual(attachments,1)
        XCTAssertTrue(ConversationActivityIcon.image("terminal") === ConversationActivityIcon.image("terminal"))
        for icon in ["terminal","image","files","search","tool"] {XCTAssertNotNil(ConversationActivityIcon.image(icon).cgImage(forProposedRect:nil,context:nil,hints:nil))}
    }
    @MainActor func testMathInvalidationTouchesOnlyDependentBlocksAndRangesSurviveEdits()throws {
        let scroll=NSScrollView(),text=NSTextView();scroll.documentView=text
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        var history=try [message("1","First 🌊 $x$"),message("2","Unaffected"),message("3","Third $y$")]
        func render(_ thread:String="a") {_=renderer.render(history,threadID:thread,in:text,scroll:scroll,renderMath:true,palette:[.white])}
        render();let original=text.string
        renderer.invalidateMath("y");render()
        XCTAssertEqual(renderer.lastReplacedBlockCount,1);XCTAssertEqual(text.string,original)
        history[0]=try message("1","Longer first message $x$ 🌊🌊")
        render();XCTAssertEqual(renderer.lastReplacedBlockCount,1)
        renderer.invalidateMath("y");render();XCTAssertEqual(renderer.lastReplacedBlockCount,1)
        history.removeLast();render();XCTAssertFalse(text.string.contains("Third"))
        history.append(try message("4","New final"));render();XCTAssertTrue(text.string.hasSuffix("New final\n"))
        history=[try message("5","New thread")];render("b")
        XCTAssertEqual(text.string,"New thread\n")
        renderer.invalidateMath("x");XCTAssertFalse(renderer.hasDirtyMath)
    }
}
