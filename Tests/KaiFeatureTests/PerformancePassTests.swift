import AppKit
import ImageIO
import XCTest
@testable import NotchPrototype

final class PerformancePassTests:XCTestCase {
    func testActivityUpdatesDoNotRepeatedlyWakeSoundPlayback(){
        var gate=ActivitySoundGate()
        XCTAssertTrue(gate.shouldPlay("thinking",at:0))
        for update in 1...1000 {
            XCTAssertFalse(gate.shouldPlay(["thinking","waiting","working","terminal","search"][update % 5],at:Double(update)))
        }
        XCTAssertTrue(gate.shouldPlay("success",at:1001))
        XCTAssertFalse(gate.shouldPlay("success",at:1002))
        XCTAssertTrue(gate.shouldPlay("permission",at:1003))
        XCTAssertFalse(gate.shouldPlay("question",at:1004))
        XCTAssertTrue(gate.shouldPlay("listening",at:1005))
        XCTAssertFalse(gate.shouldPlay("listening",at:1006))
    }
    func testActivityFlickerIsRateLimitedWhileImportantCuesRemainImmediate(){
        var gate=ActivitySoundGate()
        XCTAssertTrue(gate.shouldPlay("working",at:0))
        XCTAssertFalse(gate.shouldPlay("off",at:0.1))
        XCTAssertFalse(gate.shouldPlay("thinking",at:0.2))
        XCTAssertTrue(gate.shouldPlay("success",at:0.3))
        XCTAssertTrue(gate.shouldPlay("failure",at:0.4))
        XCTAssertFalse(gate.shouldPlay("working",at:0.5))
        XCTAssertFalse(gate.shouldPlay("off",at:2))
        XCTAssertTrue(gate.shouldPlay("working",at:3))
    }
    @MainActor func testHiddenIndicatorHasNoPerpetualShineAnimations(){
        let recipe=GradientRecipe(angleDegrees:0,cycleDurationMs:2000,stops:[GradientStop(location:0,color:"#ffffff")])
        let profile=PetGradientProfile(palette:PetPalette(shadow:"#000000",primary:"#ffffff",secondary:"#ffffff",accent:"#ffffff",highlight:"#ffffff",foreground:"#ffffff"),gradients:PetGradients(ambient:recipe,thinking:recipe,working:recipe,success:recipe,warning:recipe,error:recipe))
        let indicator=NotchIndicatorEngine()
        func descendants(_ layer:CALayer)->[CALayer]{[layer]+(layer.sublayers ?? []).flatMap(descendants)}
        func shineAnimations()->[CAAnimation]{(descendants(indicator.layer)+descendants(indicator.fileLayer)).compactMap{$0.animation(forKey:"shine")}}
        indicator.setProfile(profile,wave:false)
        XCTAssertTrue(shineAnimations().isEmpty)
        indicator.showThinking();indicator.setProfile(profile,wave:false)
        XCTAssertTrue(shineAnimations().allSatisfy{$0.repeatCount == 0})
        indicator.hide()
        XCTAssertTrue(shineAnimations().isEmpty)
    }
    private func state(_ fields:[String:Any]=[:])throws->KaiState {
        var object:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadId":"chat","threadName":"Chat","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[],"notice":"","attachments":[]]
        object.merge(fields){_,new in new}
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
    }

    @MainActor func testStreamingStateDoesNotRelayoutOrRecreateStableControls()throws {
        let ui=MiniAppInterface();ui.layout(in:CGRect(x:0,y:0,width:370,height:257))
        let initial=try state();ui.setState(initial)
        func identities(_ layer:CALayer)->[ObjectIdentifier] {
            [ObjectIdentifier(layer)]+(layer.sublayers ?? []).flatMap(identities)
        }
        let original=identities(ui.layer)
        var composerLayouts=0,conversationLayouts=0
        ui.composerFrameChanged={_,_ in composerLayouts += 1}
        ui.conversationFrameChanged={_ in conversationLayouts += 1}
        for revision in 0..<100 {
            ui.setState(try state(["history":[["id":"stream","kind":"assistant","lines":[["style":"","text":"Revision \(revision)"]]]]]))
        }
        XCTAssertEqual(composerLayouts,0);XCTAssertEqual(conversationLayouts,0)
        XCTAssertEqual(identities(ui.layer),original)
        ui.setState(try state(["approval":true]))
        XCTAssertNotEqual(identities(ui.layer),original,"Approval actions must still update")
    }

    func testSpriteTimelinePreservesUnequalFrameDurationsAndFinalFrame()throws {
        let timeline=try XCTUnwrap(SpriteTimeline(row:2,rows:10,columns:[0,3,7],durationsMs:[100,250,150],loops:false))
        XCTAssertEqual(timeline.duration,0.5,accuracy:0.00001)
        XCTAssertEqual(timeline.keyTimes.map(\.doubleValue),[0,0.2,0.7,1])
        XCTAssertEqual(timeline.frames[1],CGRect(x:3.0/8,y:0.7,width:1.0/8,height:0.1))
        let animation=timeline.animation()
        XCTAssertEqual(animation.calculationMode,.discrete)
        XCTAssertEqual(animation.repeatCount,0)
        XCTAssertEqual((animation.values?.last as? NSValue)?.rectValue,timeline.frames.last)
        let loop=try XCTUnwrap(SpriteTimeline(row:0,rows:1,columns:[0,1],durationsMs:[50,50],loops:true))
        XCTAssertEqual(loop.animation().repeatCount,.infinity)
        XCTAssertEqual((loop.animation().values?.last as? NSValue)?.rectValue,loop.frames.first)
        XCTAssertNil(SpriteTimeline(row:0,rows:0,columns:[0],durationsMs:[100],loops:true))
        XCTAssertNil(SpriteTimeline(row:1,rows:1,columns:[0],durationsMs:[100],loops:true))
        XCTAssertNil(SpriteTimeline(row:0,rows:1,columns:[8],durationsMs:[100],loops:true))
        XCTAssertNil(SpriteTimeline(row:0,rows:1,columns:[0],durationsMs:[],loops:true))
    }

    @MainActor func testLargeUnchangedTranscriptDoesNotEditStorage()throws {
        let objects=(0..<2000).map{["id":"message-\($0)","kind":"assistant","lines":[["style":"","text":"Message \($0): a representative response with Unicode 🌊 and streamed text."]]] as [String:Any]}
        let data=try JSONSerialization.data(withJSONObject:objects)
        let history=try JSONDecoder().decode([KaiMessage].self,from:data)
        let refreshed=try JSONDecoder().decode([KaiMessage].self,from:data)
        let renderer=ConversationRenderer(mathRenderer:MathRenderer()),text=NSTextView(),scroll=NSScrollView()
        scroll.documentView=text
        _=renderer.render(history,threadID:"chat",in:text,scroll:scroll,renderMath:false,palette:[.white])
        text.setSelectedRange(NSRange(location:3,length:5))
        var edits=0
        let observer=NotificationCenter.default.addObserver(forName:NSTextStorage.didProcessEditingNotification,object:text.textStorage,queue:nil){_ in edits += 1}
        defer{NotificationCenter.default.removeObserver(observer)}
        let start=ProcessInfo.processInfo.systemUptime
        for _ in 0..<100 {
            _=renderer.render(refreshed,threadID:"chat",in:text,scroll:scroll,renderMath:false,palette:[.white])
            XCTAssertFalse(renderer.didChange)
        }
        let elapsed=ProcessInfo.processInfo.systemUptime-start
        XCTAssertEqual(edits,0);XCTAssertEqual(text.selectedRange(),NSRange(location:3,length:5))
        print(String(format:"PERF: 2,000-message unchanged render: %.3f ms/update (100 updates)",elapsed*10))
        // Keep the previous signature-building path as a reproducible comparison.
        let legacyStart=ProcessInfo.processInfo.systemUptime
        var count=0
        for _ in 0..<100 {
            let signatures=refreshed.map{block in block.kind+"\u{1f}"+(block.activityIcon ?? "")+"\u{1f}"+block.id+"\u{1f}"+block.lines.map{$0.style+"\u{1e}"+$0.text+"\u{1e}"+String($0.mathEnabled ?? true)}.joined(separator:"\u{1f}")}
            count += signatures.count
        }
        XCTAssertEqual(count,200000)
        print(String(format:"PERF: previous signature allocation alone: %.3f ms/update",(ProcessInfo.processInfo.systemUptime-legacyStart)*10))
    }

    @MainActor func testGalleryRemovesFrameAnimationsWhenHidden() {
        let gallery=PetGallery()
        let motion=PetGalleryMotion(row:0,columns:[0,1],durationsMs:[100,200],loop:true)
        gallery.configure([PetGallerySpec(id:"pet",spritesheet:"missing.png",rows:1,motions:["idle":motion],colors:[])],at:URL(fileURLWithPath:"/tmp"))
        gallery.show(selected:"pet",motion:"idle")
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            XCTAssertTrue(gallery.layer.sublayers?.contains{$0.animation(forKey:"spriteFrames") != nil} == true)
        }
        gallery.hide()
        XCTAssertFalse(gallery.layer.sublayers?.contains{$0.animation(forKey:"spriteFrames") != nil} == true)
    }

    func testArtworkDecodeDownsamplesBeforeDisplayAndRejectsInvalidData()throws {
        let bitmap=try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:2048,pixelsHigh:1024,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0))
        let data=try XCTUnwrap(bitmap.representation(using:.png,properties:[:]))
        let decoded=try XCTUnwrap(MediaNowPlaying.decodeArtwork(data))
        XCTAssertLessThanOrEqual(max(decoded.width,decoded.height),256)
        XCTAssertEqual(Double(decoded.width)/Double(decoded.height),2,accuracy:0.01)
        XCTAssertLessThanOrEqual(decoded.bytesPerRow*decoded.height,256*256*4)
        XCTAssertNotNil(MediaNowPlaying.profile(decoded))
        XCTAssertNil(MediaNowPlaying.decodeArtwork(Data("invalid".utf8)))
        XCTAssertNil(MediaNowPlaying.decodeArtwork(Data(repeating:0,count:8*1024*1024+1)))
    }

    func testAttachmentEncodingPreservesOriginalPixels()throws {
        let bitmap=try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:1024,pixelsHigh:512,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0))
        let image=NSImage(cgImage:try XCTUnwrap(bitmap.cgImage),size:CGSize(width:512,height:256))
        let snapshot=AttachmentImageEncoding.Input(image)
        image.size=CGSize(width:1,height:1)
        let data=try XCTUnwrap(snapshot.png())
        let decoded=try XCTUnwrap(NSBitmapImageRep(data:data))
        XCTAssertEqual(decoded.pixelsWide,1024);XCTAssertEqual(decoded.pixelsHigh,512)
        XCTAssertNil(AttachmentImageEncoding.png(NSImage(size:.zero)))
    }

    func testLazyMathDetectionIncludesMultilineEquationsAndExcludesCode() {
        XCTAssertFalse(MathExpression.containsRenderableMath([KaiLine(text:"Ordinary text",style:"")]))
        XCTAssertFalse(MathExpression.containsRenderableMath([KaiLine(text:"$x$",style:"code")]))
        XCTAssertFalse(MathExpression.containsRenderableMath([KaiLine(text:"$x$",style:"",mathEnabled:false)]))
        XCTAssertTrue(MathExpression.containsRenderableMath([KaiLine(text:"Inline $x$",style:"")]))
        XCTAssertTrue(MathExpression.containsRenderableMath([KaiLine(text:"$$",style:""),KaiLine(text:"x^2",style:""),KaiLine(text:"$$",style:"")]))
    }

    func testSinglePassDecoderPreservesMixedEventOrderAndRejectsUnknownPackets()throws {
        let current=try state()
        let json=try JSONSerialization.data(withJSONObject:["project":current.project,"projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadName":"🌊","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[],"notice":"","attachments":[]])
        var packet=Data("{\"type\":\"voiceReset\",\"sessionId\":\"s\"}\n".utf8)
        packet.append(json);packet.append(10)
        packet.append(Data("{\"type\":\"voiceSDP\",\"sessionId\":\"s\",\"sdp\":\"answer\"}\n{\"type\":\"unknown\"}\n".utf8))
        let events=KaiStateStreamDecoder().receiveEvents(packet)
        XCTAssertEqual(events.count,3)
        guard case .reset("s")=events[0],case .state(let state)=events[1],case .sdp("s","answer")=events[2] else{return XCTFail("Event order changed")}
        XCTAssertEqual(state.threadName,"🌊")
    }
}
