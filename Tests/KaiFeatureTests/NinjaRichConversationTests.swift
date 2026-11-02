import AppKit
import XCTest
@testable import NotchPrototype

final class NinjaRichConversationTests:XCTestCase {
    @MainActor func testNinjaInlineImageAndHostLinkSurviveFormattingAndIncrementalRendering()throws {
        let pixels=try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:32,pixelsHigh:16,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0))
        let png=try XCTUnwrap(pixels.representation(using:.png,properties:[:]))
        let target="kai-ninja-file://open?path=%2Fremote%2Fresult.png"
        let line=KaiLine(text:"Generated result",style:"text",previewPNG:png.base64EncodedString(),linkTarget:target)
        XCTAssertEqual(ConversationInteraction.lines([line]),[line])
        let view=NSTextView(frame:CGRect(x:0,y:0,width:360,height:300)),scroll=NSScrollView(frame:CGRect(x:0,y:0,width:360,height:300));scroll.documentView=view
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        let message=KaiMessage(id:"image",kind:"assistant",lines:[line],speechText:nil,activityIcon:nil,agent:"ninja")
        _=renderer.render([message],threadID:"personal",in:view,scroll:scroll,renderMath:false,palette:[.cyan])
        let storage=try XCTUnwrap(view.textStorage)
        var imageAttachment:NSTextAttachment?
        storage.enumerateAttribute(.attachment,in:NSRange(location:0,length:storage.length)){value,range,_ in
            if let a=value as? NSTextAttachment,a.bounds.width>20 {imageAttachment=a;XCTAssertEqual((storage.attribute(.link,at:range.location,effectiveRange:nil) as? URL)?.absoluteString,target)}
        }
        XCTAssertNotNil(imageAttachment);XCTAssertLessThanOrEqual(imageAttachment!.bounds.width,280)
        _=renderer.render([message],threadID:"personal",in:view,scroll:scroll,renderMath:false,palette:[.cyan]);XCTAssertEqual(renderer.lastReplacedBlockCount,0)
    }
    @MainActor func testHostFileSpansUseUTF16AndKeepTheExactRemoteTarget()throws {
        let line=KaiLine(text:"📁 report ready",style:"text",links:[KaiInlineLink(location:3,length:6,target:"kai-ninja-file://open?path=%2Fremote%2Freport.pdf")])
        XCTAssertEqual(ConversationInteraction.lines([line]),[line])
        let view=NSTextView(),scroll=NSScrollView();scroll.documentView=view
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        _=renderer.render([KaiMessage(id:"file",kind:"assistant",lines:[line],speechText:nil,activityIcon:nil)],threadID:"personal",in:view,scroll:scroll,renderMath:false,palette:[.white])
        let range=(view.string as NSString).range(of:"report")
        XCTAssertEqual((view.textStorage?.attribute(.link,at:range.location,effectiveRange:nil) as? URL)?.scheme,"kai-ninja-file")
    }
}
