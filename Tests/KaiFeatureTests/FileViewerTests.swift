import AppKit
import XCTest
@testable import NotchPrototype

final class FileViewerTests:XCTestCase {
    @MainActor func testHoverBackdropHighlightsOneButtonAndFadesOut(){
        let backdrop=CALayer(),targets=[(CGRect(x:0,y:0,width:11,height:12),"reveal"),(CGRect(x:13,y:0,width:11,height:12),"copy")]
        FileActionSymbols.updateHover("reveal",targets:targets,backdrop:backdrop)
        XCTAssertEqual(backdrop.opacity,1);XCTAssertLessThanOrEqual(backdrop.frame.maxX,12)
        XCTAssertEqual(backdrop.frame.midX,targets[0].0.midX);XCTAssertEqual(backdrop.frame.midY,targets[0].0.midY)
        XCTAssertEqual(backdrop.cornerRadius,backdrop.frame.height/2)
        FileActionSymbols.updateHover("copy",targets:targets,backdrop:backdrop)
        XCTAssertEqual(backdrop.opacity,1);XCTAssertGreaterThanOrEqual(backdrop.frame.minX,12)
        XCTAssertEqual(backdrop.frame.midX,targets[1].0.midX);XCTAssertEqual(backdrop.frame.midY,targets[1].0.midY)
        FileActionSymbols.updateHover(nil,targets:targets,backdrop:backdrop)
        XCTAssertEqual(backdrop.opacity,0)
        XCTAssertNotNil(backdrop.animation(forKey:"fileActionHover"))
    }
    @MainActor func testCitationHoverUsesTheFullPaddedLinkAndClearsOnExit()throws {
        let view=FileActionHoverTextView(frame:CGRect(x:0,y:0,width:400,height:150)),scroll=NSScrollView()
        let renderer=ConversationRenderer(mathRenderer:MathRenderer())
        let message=KaiMessage(id:"hover",kind:"assistant",lines:[KaiLine(text:"</tmp/hover-report.md>",style:"text")],speechText:nil,activityIcon:nil)
        _=renderer.render([message],threadID:"hover",in:view,scroll:scroll,renderMath:false,palette:[.white])
        let storage=try XCTUnwrap(view.textStorage),manager=try XCTUnwrap(view.layoutManager),container=try XCTUnwrap(view.textContainer)
        manager.ensureLayout(for:container)
        var found:NSRange?,url:URL?
        storage.enumerateAttribute(.link,in:NSRange(location:0,length:storage.length)){value,range,_ in
            if let link=value as? URL,link.scheme == "kai-file-action",link.host == "copy"{found=range;url=link}
        }
        let range=try XCTUnwrap(found),glyphs=manager.glyphRange(forCharacterRange:range,actualCharacterRange:nil)
        let rect=manager.boundingRect(forGlyphRange:glyphs,in:container).offsetBy(dx:view.textContainerOrigin.x,dy:view.textContainerOrigin.y)
        view.updateFileActionHover(at:CGPoint(x:rect.midX,y:rect.midY))
        XCTAssertEqual(view.hoveredFileAction,url)
        var attachmentRange:NSRange?
        storage.enumerateAttribute(.attachment,in:range){value,range,_ in if value is NSTextAttachment{attachmentRange=range}}
        let attachment=try XCTUnwrap(attachmentRange)
        let icon=manager.boundingRect(forGlyphRange:manager.glyphRange(forCharacterRange:attachment,actualCharacterRange:nil),in:container).offsetBy(dx:view.textContainerOrigin.x,dy:view.textContainerOrigin.y)
        let highlight=try XCTUnwrap(view.fileActionHoverRect)
        XCTAssertEqual(highlight.midX,icon.midX,accuracy:0.01)
        XCTAssertEqual(highlight.midY,icon.midY,accuracy:0.01)
        XCTAssertEqual(highlight.size,CGSize(width:20,height:20))
        XCTAssertNil(manager.temporaryAttribute(.backgroundColor,atCharacterIndex:range.location,effectiveRange:nil))
        // Hovering the forgiving spacer hit area must not move the painted shape.
        view.updateFileActionHover(at:CGPoint(x:rect.minX+1,y:rect.midY))
        XCTAssertEqual(view.hoveredFileAction,url)
        XCTAssertEqual(view.fileActionHoverRect,highlight)
        view.updateFileActionHover(at:nil)
        XCTAssertNil(view.hoveredFileAction)
        XCTAssertNil(view.fileActionHoverRect)
        XCTAssertNil(manager.temporaryAttribute(.backgroundColor,atCharacterIndex:range.location,effectiveRange:nil))
    }
    @MainActor func testFileConfirmationFadesBackAndOldTimersCannotClearAnotherClick(){
        let url=URL(fileURLWithPath:"/tmp/feedback-\(UUID().uuidString).md")
        let first=FileActionSymbols.confirm("copy",url:url)
        XCTAssertEqual(FileActionSymbols.feedbackProgress("copy",url:url,now:first),0)
        XCTAssertEqual(FileActionSymbols.feedbackProgress("copy",url:url,now:first+0.2),1,accuracy:0.001)
        XCTAssertEqual(FileActionSymbols.feedbackProgress("copy",url:url,now:first+1.2),0.5,accuracy:0.001)
        XCTAssertEqual(FileActionSymbols.feedbackProgress("copy",url:url,now:first+1.4),0)
        XCTAssertEqual(FileActionSymbols.feedbackProgress("reveal",url:url,now:first+0.2),0)
        let second=FileActionSymbols.confirm("copy",url:url)
        FileActionSymbols.clear("copy",url:url,stamp:first)
        XCTAssertEqual(FileActionSymbols.confirmation("copy",url:url),second)
        FileActionSymbols.clear("copy",url:url,stamp:second)
        XCTAssertNil(FileActionSymbols.confirmation("copy",url:url))
        XCTAssertEqual(FileActionSymbols.image("copy",url:url,padded:true)?.size,CGSize(width:24,height:22))
    }
    @MainActor func testLongTextScrollsAndRepeatedLayoutDoesNotJumpToTop()throws {
        let viewer=FileViewerContent(frame:CGRect(x:0,y:0,width:180,height:140))
        let panel=NotchPanel(contentRect:viewer.frame,styleMask:[.borderless],backing:.buffered,defer:false)
        panel.isReleasedWhenClosed=false;panel.contentView=viewer
        defer{panel.close()}
        let url=URL(fileURLWithPath:"/tmp/report.md")
        let text=(0..<500).map{"Line \($0): a long file preview"}.joined(separator:"\n")
        viewer.show(text,url:url)
        let document=try XCTUnwrap(viewer.documentView as? NSTextView)
        document.layoutManager?.ensureLayout(for:document.textContainer!)
        XCTAssertEqual(viewer.displayedText,text)
        XCTAssertGreaterThan(document.frame.height,viewer.contentSize.height)
        document.scrollRangeToVisible(NSRange(location:(text as NSString).length,length:0))
        let offset=viewer.contentView.bounds.origin.y
        XCTAssertGreaterThan(offset,0)
        viewer.show(text,url:url)
        XCTAssertEqual(viewer.contentView.bounds.origin.y,offset)
        viewer.show("Another file",url:URL(fileURLWithPath:"/tmp/other.md"))
        XCTAssertEqual(viewer.contentView.bounds.origin.y,0)
    }
    @MainActor func testDoubleClickPreviewUsesDisplayedFileIdentity()throws {
        let viewer=FileViewerContent(frame:CGRect(x:0,y:0,width:180,height:140))
        let url=URL(fileURLWithPath:"/tmp/report with spaces.md")
        viewer.show("Preview",url:url)
        var opened=[URL]();viewer.onFileActions={opened.append($0)}
        let event=try XCTUnwrap(NSEvent.mouseEvent(with:.leftMouseDown,location:.zero,modifierFlags:[],timestamp:0,windowNumber:0,context:nil,eventNumber:0,clickCount:2,pressure:1))
        viewer.documentView?.mouseDown(with:event)
        XCTAssertEqual(opened,[url])
    }
    @MainActor func testCopyFileWritesAFileReferenceAndFinderActionKeepsExactURL()throws {
        _=NSApplication.shared
        let url=FileManager.default.temporaryDirectory.appendingPathComponent("Kai preview \(UUID().uuidString).md")
        try Data("report".utf8).write(to:url);defer{try? FileManager.default.removeItem(at:url)}
        let actions=FileActionsMenu(),board=NSPasteboard.withUniqueName()
        defer{board.releaseGlobally()}
        XCTAssertTrue(actions.copy(url,to:board))
        let copied=board.readObjects(forClasses:[NSURL.self],options:[.urlReadingFileURLsOnly:true]) as? [URL]
        XCTAssertEqual(copied,[url])
        var revealed=[URL](),feedback=[String]();actions.reveal={revealed=$0};actions.onPerformed={action,file in XCTAssertEqual(file,url);feedback.append(action)}
        let menu=actions.menu(for:url)
        XCTAssertEqual(menu.items.map(\.title),["Copy file","Show in Finder"])
        menu.performActionForItem(at:1)
        XCTAssertEqual(revealed,[url])
        XCTAssertEqual(feedback,["reveal"])
        XCTAssertTrue(actions.perform("copy",url:url,pasteboard:board))
        XCTAssertEqual(feedback,["reveal","copy"])
        XCTAssertFalse(actions.perform("copy",url:URL(fileURLWithPath:"/nonexistent/kai-feedback.md"),pasteboard:board))
        XCTAssertEqual(feedback,["reveal","copy"])
        XCTAssertTrue(actions.menu(for:URL(fileURLWithPath:"/nonexistent/kai-file.md")).items.allSatisfy{!$0.isEnabled})
    }
    @MainActor func testPreviewLoadsBeyondOldPrefixAndRoutesDoubleClick()async throws {
        let url=FileManager.default.temporaryDirectory.appendingPathComponent("Kai long \(UUID().uuidString).md")
        let text=String(repeating:"File content\n",count:2000)+"END OF REPORT"
        try Data(text.utf8).write(to:url);defer{try? FileManager.default.removeItem(at:url)}
        let ui=MiniAppInterface();ui.layout(in:CGRect(x:0,y:0,width:500,height:400));ui.setVisible(true,duration:0)
        let ready=expectation(description:"full text loaded")
        var frame=CGRect.zero
        ui.previewContentChanged={rect,value,file,visible in
            if file==url,value==text,visible {frame=rect;ready.fulfill()}
        }
        ui.previewFile(url)
        await fulfillment(of:[ready],timeout:5)
        var direct=[String]();ui.onFileAction={action,file in XCTAssertEqual(file,url);direct.append(action)}
        XCTAssertTrue(ui.handleClick(CGPoint(x:frame.maxX-32.5,y:frame.maxY+8)))
        XCTAssertTrue(ui.handleClick(CGPoint(x:frame.maxX-19.5,y:frame.maxY+8)))
        XCTAssertEqual(direct,["reveal","copy"])
        XCTAssertEqual(ui.fileAction(at:CGPoint(x:frame.maxX-32.5,y:frame.maxY+8))?.action,"reveal")
        XCTAssertEqual(ui.fileAction(at:CGPoint(x:frame.maxX-19.5,y:frame.maxY+8))?.action,"copy")
        XCTAssertNil(ui.fileAction(at:CGPoint(x:frame.midX,y:frame.midY)))
        var actions=[URL]();ui.onFileActions={actions.append($0)}
        XCTAssertTrue(ui.handleClick(CGPoint(x:frame.midX,y:frame.midY),count:2))
        XCTAssertEqual(actions,[url])
    }
}
