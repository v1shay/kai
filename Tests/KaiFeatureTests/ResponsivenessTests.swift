import AppKit
import XCTest
@testable import NotchPrototype

final class ResponsivenessTests:XCTestCase {
    private func state(projects:Int=0,chats:Int=0)throws->Data {
        let obj:[String:Any]=["project":"/project/0","projects":(0..<projects).map{["path":"/project/\($0)","name":"Project \($0)"]},"chats":(0..<chats).map{["id":"chat-\($0)","name":"Chat \($0)","status":"idle"]},"standaloneChats":[],"standalone":false,"threadName":"Hello 🌊","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[],"notice":"","attachments":[]]
        return try JSONSerialization.data(withJSONObject:obj)
    }
    func testLargeSidebarCreatesOnlyViewportRowsAndCanReachLastProject()async throws {
        let state=try JSONDecoder().decode(KaiState.self,from:state(projects:10000,chats:10000))
        await MainActor.run {
            for size in [CGSize(width:354,height:257),CGSize(width:708,height:514),CGSize(width:500,height:700)] {
                let ui=MiniAppInterface();ui.layout(in:CGRect(origin:.zero,size:size));ui.setState(state)
                XCTAssertLessThan(ui.renderedSidebarRowCount,Int(size.height/16)+10)
                ui.scroll(-1_000_000,at:CGPoint(x:10,y:100))
                XCTAssertLessThan(ui.renderedSidebarRowCount,Int(size.height/16)+10)
                var selected=[String]()
                ui.onProject={selected.append($0)}
                for y in stride(from:CGFloat(35),to:size.height-20,by:3) {_=ui.handleClick(CGPoint(x:10,y:y))}
                XCTAssertTrue(selected.contains("/project/9999"))
                for _ in 0..<10 {ui.setState(state)}
                XCTAssertLessThan(ui.renderedSidebarRowCount,Int(size.height/16)+10)
            }
        }
    }
    func testPetThemeChangesPreserveTextAttachmentSelectionAndViewport()async {
        await MainActor.run {
            for size in [CGSize(width:354,height:257),CGSize(width:708,height:514)] {
                let scroll=NSScrollView(frame:CGRect(origin:.zero,size:size)),text=NSTextView(frame:CGRect(x:0,y:0,width:size.width,height:20000))
                scroll.documentView=text;text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true
                let content=NSMutableAttributedString(string:String(repeating:"Text 🌊 with a link and math\n",count:300),attributes:[.font:NSFont.systemFont(ofSize:9),.kaiGradientPosition:CGFloat(0.5),.foregroundColor:NSColor.blue])
                let attachment=NSTextAttachment();attachment.image=NSImage(size:CGSize(width:20,height:12));content.append(NSAttributedString(attachment:attachment))
                content.addAttribute(.link,value:URL(string:"https://example.com")!,range:NSRange(location:0,length:4))
                text.textStorage!.setAttributedString(content);text.layoutManager?.ensureLayout(for:text.textContainer!)
                text.setSelectedRange(NSRange(location:5,length:3));scroll.contentView.scroll(to:CGPoint(x:0,y:200))
                let original=text.string,viewport=scroll.contentView.bounds.origin,selection=text.selectedRange(),length=text.textStorage!.length
                for palette in [[NSColor.red,.orange,.yellow],[NSColor.blue,.cyan,.green],[NSColor.purple,.white,.red]] {
                    ConversationTheme.recolor(text.textStorage!,palette:palette)
                    text.layoutManager?.ensureLayout(for:text.textContainer!)
                    XCTAssertEqual(text.string,original);XCTAssertEqual(text.textStorage!.length,length)
                    XCTAssertEqual(text.selectedRange(),selection);XCTAssertEqual(scroll.contentView.bounds.origin,viewport)
                    XCTAssertTrue((text.textStorage!.attribute(.attachment,at:length-1,effectiveRange:nil) as? NSTextAttachment) === attachment)
                    XCTAssertNotNil(text.textStorage!.attribute(.link,at:0,effectiveRange:nil))
                }
            }
        }
    }
    @MainActor func testSpriteCacheCoalescesLoadsAndReportsMissingAssets()async throws {
        let root=URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let catalog=try JSONSerialization.jsonObject(with:Data(contentsOf:root.appendingPathComponent("kai_pets/animation_catalog.json"))) as! [String:Any]
        let pet=(catalog["pets"] as! [[String:Any]])[0]
        let url=root.appendingPathComponent("kai_pets/"+(pet["spritesheet"] as! String))
        let cache=SpriteAssetCache(),done=expectation(description:"coalesced sprite load"),missing=expectation(description:"missing asset completes")
        done.expectedFulfillmentCount=2
        var images=[CGImage]()
        for _ in 0..<2 {cache.load(url){image in if let image{images.append(image)};done.fulfill()}}
        cache.load(root.appendingPathComponent("missing-pet.png")){image in XCTAssertNil(image);missing.fulfill()}
        await fulfillment(of:[done,missing],timeout:5)
        XCTAssertEqual(images.count,2)
        if images.count == 2 {XCTAssertTrue(images[0] === images[1]);XCTAssertTrue(cache.image(at:url) === images[0])}
    }
    func testOversizedStateLineIsDiscardedAndNextPacketRecovers()throws {
        let json=try state()
        let decoder=KaiStateStreamDecoder(maximumLineBytes:json.count+10)
        XCTAssertTrue(decoder.receive(Data(repeating:65,count:json.count+20)).isEmpty)
        var packet=Data("discarded suffix\n".utf8);packet.append(json);packet.append(10)
        XCTAssertEqual(decoder.receive(packet).count,1)
    }
    func testStateDecoderHandlesChunkedUnicodeAndMalformedLines()throws {
        var packet=Data("invalid\n".utf8);let json=try state(projects:1000)
        packet.append(json);packet.append(10);packet.append(json);packet.append(10)
        let decoder=KaiStateStreamDecoder()
        var states=[KaiState]()
        for offset in stride(from:0,to:packet.count,by:37) {states += decoder.receive(packet.subdata(in:offset..<min(offset+37,packet.count)))}
        XCTAssertEqual(states.count,2);XCTAssertEqual(states.last?.threadName,"Hello 🌊");XCTAssertEqual(states.last?.projects.count,1000)
    }
}
