import AppKit
import XCTest
@testable import NotchPrototype

final class ConversationTextRevealTests:XCTestCase {
    @MainActor func testStandardFieldEditorCanRevealWithoutMutatingTextOrCaret()throws{
        _=NSApplication.shared
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{throw XCTSkip("Reduced Motion disables text reveal")}
        let text=NSTextView(frame:CGRect(x:0,y:0,width:360,height:80)),scroll=NSScrollView(frame:CGRect(x:0,y:0,width:360,height:80))
        text.font = .systemFont(ofSize:13);text.string="Typed prefix and new dictated words 🐈";text.textColor = .white;scroll.documentView=text
        if let container=text.textContainer{text.layoutManager?.ensureLayout(for:container)}
        let range=NSRange(location:13,length:20);text.setSelectedRange(NSRange(location:(text.string as NSString).length,length:0));let caret=text.selectedRange(),original=text.textStorage!.copy() as! NSAttributedString
        let reveal=ConversationTextReveal();reveal.reveal([range],in:text,loading:false)
        XCTAssertGreaterThan(reveal.snapshotCount,0);XCTAssertEqual(text.textStorage!,original);XCTAssertEqual(text.selectedRange(),caret)
        reveal.finish();XCTAssertNil(text.layoutManager?.temporaryAttribute(.foregroundColor,atCharacterIndex:range.location,effectiveRange:nil));XCTAssertEqual(text.textStorage!,original)
    }
    private func message(_ text:String,kind:String="assistant")throws->KaiMessage {
        try JSONDecoder().decode(KaiMessage.self,from:JSONSerialization.data(withJSONObject:["id":"answer","kind":kind,"lines":[["text":text,"style":""]]]))
    }
    func testDisplayDiffOnlyRevealsChangedSuffixAndKeepsComposedCharactersWhole(){
        XCTAssertEqual(TextRevealChange.suffix(old:"Hello\n",new:"Hello world\n"),NSRange(location:5,length:7))
        XCTAssertNil(TextRevealChange.suffix(old:"same",new:"same"))
        XCTAssertNil(TextRevealChange.suffix(old:"longer",new:"long"))
        let emoji="👩🏽‍💻"
        XCTAssertEqual(TextRevealChange.suffix(old:emoji+" old",new:emoji+" new"),NSRange(location:emoji.utf16.count+1,length:3))
        XCTAssertEqual(TextRevealChange.suffix(old:"👩🏽‍💻",new:"👩🏻‍💻"),NSRange(location:0,length:"👩🏻‍💻".utf16.count))
    }
    func testViewportBudgetDoesNotScaleWithHistoryLength(){
        let result=TextRevealChange.visible([NSRange(location:0,length:1_000_000)],in:NSRange(location:100_000,length:4000))
        XCTAssertEqual(result,[NSRange(location:100_000,length:512)])
        XCTAssertTrue(TextRevealChange.visible([NSRange(location:0,length:100)],in:NSRange(location:1000,length:500)).isEmpty)
    }
    func testSettingDefaultsOffAndPersists(){
        let name="TextReveal-"+UUID().uuidString,defaults=UserDefaults(suiteName:name)!
        defer{defaults.removePersistentDomain(forName:name)}
        let preferences=TextRevealPreferences(defaults:defaults)
        XCTAssertFalse(preferences.enabled);preferences.enabled=true
        XCTAssertTrue(TextRevealPreferences(defaults:defaults).enabled)
    }
    @MainActor func testStreamingOnlyAnimatesSuffixAndUnchangedPacketsDoNoWork()throws {
        let renderer=ConversationRenderer(mathRenderer:MathRenderer()),scroll=NSScrollView(),text=NSTextView()
        scroll.documentView=text;var changes=0;renderer.willChange={changes += 1}
        _=renderer.render([try message("Hello")],threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white],collectRevealRanges:true)
        XCTAssertTrue(renderer.didChangeThread);XCTAssertEqual(renderer.revealRanges,[NSRange(location:0,length:6)])
        _=renderer.render([try message("Hello world")],threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white],collectRevealRanges:true)
        XCTAssertFalse(renderer.didChangeThread);XCTAssertEqual(renderer.revealRanges,[NSRange(location:5,length:7)])
        let count=changes
        _=renderer.render([try message("Hello world")],threadID:"a",in:text,scroll:scroll,renderMath:false,palette:[.white],collectRevealRanges:true)
        XCTAssertEqual(changes,count);XCTAssertTrue(renderer.revealRanges.isEmpty)
        _=renderer.render([try message("user prompt",kind:"user")],threadID:"b",in:text,scroll:scroll,renderMath:false,palette:[.white],collectRevealRanges:true)
        XCTAssertTrue(renderer.didChangeThread);XCTAssertFalse(renderer.revealRanges.isEmpty)
    }
    @MainActor private func fixture()->(NSTextView,NSScrollView,InteractionLayoutManager){
        _=NSApplication.shared
        let text=NSTextView(frame:CGRect(x:0,y:0,width:360,height:240)),scroll=NSScrollView(frame:CGRect(x:0,y:0,width:360,height:180))
        text.drawsBackground=false;text.isEditable=false;text.font=NSFont.monospacedSystemFont(ofSize:15,weight:.regular)
        text.textContainerInset=CGSize(width:4,height:8)
        let manager=InteractionLayoutManager();text.textContainer?.replaceLayoutManager(manager)
        text.string="New letters briefly decode, then settle.\nThe real text stays selectable. 🌊\nLinks, emoji and equations stay native."
        text.textStorage?.addAttribute(.foregroundColor,value:NSColor.systemTeal,range:NSRange(location:0,length:(text.string as NSString).length))
        scroll.documentView=text;ConversationViewport.layout(text,in:scroll)
        return (text,scroll,manager)
    }
    @MainActor func testSnapshotsAreBoundedAndCleanupRestoresNativeGlyphs()async throws {
        let (text,scroll,manager)=fixture();_ = scroll
        if let directory=ProcessInfo.processInfo.environment["KAI_TEXT_REVEAL_RENDER"]{
            try FileManager.default.createDirectory(atPath:directory,withIntermediateDirectories:true)
            text.wantsLayer=true;text.display()
            let bitmap=try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:720,pixelsHigh:360,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0))
            let context=try XCTUnwrap(NSGraphicsContext(bitmapImageRep:bitmap)).cgContext
            context.translateBy(x:0,y:360);context.scaleBy(x:2,y:-2);text.layer?.render(in:context)
            try XCTUnwrap(bitmap.representation(using:.png,properties:[:])).write(to:URL(fileURLWithPath:directory).appendingPathComponent("baseline.png"))
        }
        let reveal=ConversationTextReveal(),original=text.textStorage!.copy() as! NSAttributedString
        let start=CACurrentMediaTime()
        reveal.reveal([NSRange(location:0,length:(text.string as NSString).length)],in:text,loading:true)
        let elapsed=CACurrentMediaTime()-start
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{XCTAssertEqual(reveal.activeTileCount,0);return}
        XCTAssertGreaterThan(reveal.activeTileCount,0)
        XCTAssertLessThanOrEqual(reveal.activeTileCount,ConversationTextReveal.maximumTiles)
        XCTAssertEqual(reveal.snapshotCount,reveal.activeTileCount*4)
        XCTAssertFalse(manager.revealGlyphRanges.isEmpty)
        XCTAssertTrue(original.isEqual(to:text.textStorage!))
        print(String(format:"TEXT REVEAL: visible viewport setup %.3f ms, %d tiles, %d snapshots",elapsed*1000,reveal.activeTileCount,reveal.snapshotCount))
        if let directory=ProcessInfo.processInfo.environment["KAI_TEXT_REVEAL_RENDER"]{
            try FileManager.default.createDirectory(atPath:directory,withIntermediateDirectories:true)
            for (index,tile) in (text.layer?.sublayers ?? []).enumerated(){
                guard let group=tile.animation(forKey:"textDecode") as? CAAnimationGroup,
                      let frames=(group.animations?.first as? CAKeyframeAnimation)?.values as? [CGImage] else{continue}
                for (phase,image) in frames.enumerated(){
                    let bitmap=NSBitmapImageRep(cgImage:image)
                    try XCTUnwrap(bitmap.representation(using:.png,properties:[:])).write(to:URL(fileURLWithPath:directory).appendingPathComponent("tile-\(index)-phase-\(phase).png"))
                }
            }
            // Render the actual layer hierarchy as well as the individual textures.
            // This catches flipped or displaced overlays that texture inspection misses.
            text.display()
            for phase in 0..<4 {
                CATransaction.begin();CATransaction.setDisableActions(true)
                for tile in text.layer?.sublayers ?? [] {
                    guard let group=tile.animation(forKey:"textDecode") as? CAAnimationGroup,
                          let frames=(group.animations?.first as? CAKeyframeAnimation)?.values as? [CGImage] else{continue}
                    tile.contents=frames[phase]
                }
                CATransaction.commit()
                let bitmap=try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:720,pixelsHigh:360,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0))
                let context=try XCTUnwrap(NSGraphicsContext(bitmapImageRep:bitmap)).cgContext
                context.translateBy(x:0,y:360);context.scaleBy(x:2,y:-2);text.layer?.render(in:context)
                try XCTUnwrap(bitmap.representation(using:.png,properties:[:])).write(to:URL(fileURLWithPath:directory).appendingPathComponent("composite-\(phase).png"))
            }
            let root=URL(fileURLWithPath:directory)
            XCTAssertEqual(try Data(contentsOf:root.appendingPathComponent("baseline.png")),try Data(contentsOf:root.appendingPathComponent("composite-3.png")),"Final overlays must match native text pixel for pixel")
        }
        try await Task.sleep(nanoseconds:500_000_000)
        XCTAssertEqual(reveal.activeTileCount,0);XCTAssertTrue(manager.revealGlyphRanges.isEmpty)
        XCTAssertFalse(text.layer?.sublayers?.contains{$0.animation(forKey:"textDecode") != nil} ?? false)
        XCTAssertTrue(original.isEqual(to:text.textStorage!))
    }
    @MainActor func testOffscreenChangesCreateNoSnapshotsAndRapidUpdatesDoNotAccumulate(){
        let (text,scroll,manager)=fixture();_ = scroll
        let reveal=ConversationTextReveal()
        reveal.reveal([NSRange(location:10_000,length:100)],in:text,loading:false)
        XCTAssertEqual(reveal.snapshotCount,0)
        let start=CACurrentMediaTime()
        for _ in 0..<30 {
            reveal.reveal([NSRange(location:0,length:8)],in:text,loading:false)
            XCTAssertLessThanOrEqual(reveal.activeTileCount,1)
        }
        print(String(format:"TEXT REVEAL: streaming suffix preparation %.3f ms/update",(CACurrentMediaTime()-start)*1000/30))
        reveal.finish();XCTAssertTrue(manager.revealGlyphRanges.isEmpty);XCTAssertEqual(reveal.activeTileCount,0)
        text.isHidden=true
        reveal.reveal([NSRange(location:0,length:8)],in:text,loading:false)
        XCTAssertEqual(reveal.snapshotCount,0)
    }
}
