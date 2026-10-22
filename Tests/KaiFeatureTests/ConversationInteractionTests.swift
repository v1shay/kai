import XCTest
import AppKit
@testable import NotchPrototype

final class ConversationInteractionTests:XCTestCase {
    func testFileTagsKeepSpacesUnicodeAndParenthesesInTheirPaths()throws {
        let path="/Users/example/Library/Application Support/Kai/Chats/outputs/Research (final)/résumé 🌊.md"
        for tag in ["<\(path)>","(<\(path)>)","<\(URL(fileURLWithPath:path).absoluteString)>"] {
            let citations=fileCitations(in:"Report \(tag) ready")
            XCTAssertEqual(citations.count,1)
            XCTAssertEqual(citations.first?.url.path,path)
            let display=displayCitations(in:"Report \(tag) ready")
            XCTAssertEqual(display.0,"Report ▧  résumé 🌊.md ready")
            XCTAssertEqual(display.1.first?.1.path,path)
        }
        XCTAssertTrue(fileCitations(in:"<script>alert(1)</script>").isEmpty)
        XCTAssertTrue(fileCitations(in:"<file://remote-host/private/report.md>").isEmpty)
    }
    @MainActor func testScreenshotFileTagRendersAsClickableDocumentWithFullLocalURL()throws {
        let path="/Users/vishayagarwal/Library/Application Support/Kai/Chats/outputs/webby-research/Webby-research-report.md"
        let view=NSTextView(frame:NSRect(x:0,y:0,width:300,height:300)),scroll=NSScrollView()
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        let message=KaiMessage(id:"file",kind:"assistant",lines:[KaiLine(text:"Webby research report (<\(path)>)",style:"text")],speechText:nil,activityIcon:nil)
        _=renderer.render([message],threadID:"thread",in:view,scroll:scroll,renderMath:false,palette:[.systemPink])
        let range=(view.string as NSString).range(of:"Webby-research-report.md")
        XCTAssertNotEqual(range.location,NSNotFound)
        XCTAssertFalse(view.string.contains("Application Support"))
        let url=try XCTUnwrap(view.textStorage?.attribute(.link,at:range.location,effectiveRange:nil) as? URL)
        XCTAssertTrue(url.isFileURL)
        XCTAssertEqual(url.path,path)
        var fileActions=[String:URL]()
        view.textStorage?.enumerateAttribute(.link,in:NSRange(location:0,length:view.textStorage!.length)){value,_,_ in
            if let link=value as? URL,link.scheme == "kai-file-action",let action=link.host {fileActions[action]=link}
        }
        XCTAssertEqual(Set(fileActions.keys),Set(["copy","reveal"]))
        for link in fileActions.values {
            XCTAssertEqual(URLComponents(url:link,resolvingAgainstBaseURL:false)?.queryItems?.first{$0.name == "path"}?.value,path)
        }
        XCTAssertEqual(view.textStorage?.attribute(.toolTip,at:range.location,effectiveRange:nil) as? String,path)
        let code=ConversationInteraction.lines([KaiLine(text:"<\(path)>",style:"code")])
        XCTAssertEqual(code.first?.text,"<\(path)>")
        XCTAssertEqual(code.first?.style,"code")
    }
    private let envelope="<send_user_message_question_reply>\n[{\"questionItemId\":\"ignored\",\"question\":\"Does it open?\",\"answer\":\"Panel opens ✓\"}]\n</send_user_message_question_reply>"
    func testReplyBecomesReadableCardAndKeepsSurroundingText(){
        let result=ConversationInteraction.lines([KaiLine(text:"Before\n"+envelope+"\nAfter",style:"text")])
        XCTAssertEqual(result.filter{$0.style.hasPrefix("interaction:")}.map(\.text),["✓  Question answered","Does it open?","Panel opens ✓"])
        XCTAssertTrue(result.contains{$0.text == "Before"});XCTAssertTrue(result.contains{$0.text == "After"})
        XCTAssertFalse(result.contains{$0.text.contains("questionItemId")})
    }
    func testRemoteOutputCitationBecomesDocumentCard(){
        let token=#":codex-file-citation{path="https://docs.google.com/presentation/d/example/edit" purpose="output"}"#
        let lines=ConversationInteraction.lines([KaiLine(text:"Created\n"+token,style:"text")])
        XCTAssertEqual(lines.filter{$0.style.hasSuffix(":document")}.count,1)
        let display=displayCitations(in:token)
        XCTAssertEqual(display.0,"▧  Google Slides  ↗")
        XCTAssertEqual(display.1.first?.1.absoluteString,"https://docs.google.com/presentation/d/example/edit")
        XCTAssertEqual(ConversationInteraction.lines([KaiLine(text:token,style:"code")]).first?.style,"code")
        let invalid=#":codex-file-citation{path="javascript:alert(1)" purpose="output"}"#
        XCTAssertTrue(fileCitations(in:invalid).isEmpty)
    }
    func testVoiceEnvelopesAndResponseMarkersBecomeReadableSections(){
        let raw="<realtime_delegation>\n<input>Hello there</input>\n<transcript_delta>Checking the result</transcript_delta>\n</realtime_delegation>\n[STATUS] Checking data\n[COMPLETE] All done"
        let lines=ConversationInteraction.lines(raw.components(separatedBy:"\n").map{KaiLine(text:$0,style:"text")})
        XCTAssertEqual(lines.map(\.text),["Hello there","Checking the result","Checking data","All done"])
        XCTAssertTrue(lines.allSatisfy{$0.style.hasPrefix("voice:")})
        XCTAssertEqual(ConversationInteraction.lines([KaiLine(text:raw,style:"code")]).first?.text,raw)
    }
    func testCodeAndMalformedDataStayIntact(){
        let code=KaiLine(text:envelope,style:"code")
        XCTAssertEqual(ConversationInteraction.lines([code]).first?.text,envelope)
        let malformed="<send_user_message_question_reply>not JSON</send_user_message_question_reply>"
        XCTAssertEqual(ConversationInteraction.lines([KaiLine(text:malformed,style:"text")]).first?.text,malformed)
    }
    @MainActor func testRendererKeepsCardsAsSelectableTextWithCopyLink(){
        let view=NSTextView(frame:NSRect(x:0,y:0,width:300,height:300)),scroll=NSScrollView()
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        let message=KaiMessage(id:"reply",kind:"user",lines:[KaiLine(text:envelope,style:"text")],speechText:nil,activityIcon:nil)
        _=renderer.render([message],threadID:"thread",in:view,scroll:scroll,renderMath:false,palette:[.systemPink])
        let range=(view.string as NSString).range(of:"Panel opens ✓")
        XCTAssertNotEqual(range.location,NSNotFound)
        XCTAssertNotNil(view.textStorage?.attribute(.kaiInteractionCard,at:range.location,effectiveRange:nil))
        let url=view.textStorage?.attribute(.link,at:range.location,effectiveRange:nil) as? URL
        XCTAssertEqual(url?.scheme,"kai-reply")
        _=renderer.render([message],threadID:"thread",in:view,scroll:scroll,renderMath:false,palette:[.systemPink])
        XCTAssertEqual(renderer.lastReplacedBlockCount,0)
    }
}
