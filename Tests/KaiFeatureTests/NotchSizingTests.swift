import XCTest
import AppKit
@testable import NotchPrototype

final class NotchSizingTests: XCTestCase {
    let baseline=CGSize(width:370,height:320)
    func testDefaultAndScreenLimits() {
        let screen=CGSize(width:1440,height:900)
        XCTAssertEqual(NotchSizing.constrained(baseline,baseline:baseline,screen:screen),baseline)
        XCTAssertEqual(NotchSizing.constrained(CGSize(width:10000,height:10000),baseline:baseline,screen:screen),CGSize(width:1360,height:820))
        XCTAssertEqual(NotchSizing.constrained(CGSize(width:CGFloat.nan,height:-20),baseline:baseline,screen:screen),baseline)
    }
    func testIndependentAndCombinedDragWithCenteredWidth() {
        let delta=CGPoint(x:50,y:-80)
        XCTAssertEqual(NotchSizing.dragged(baseline,delta:delta,mode:.width),CGSize(width:470,height:320))
        XCTAssertEqual(NotchSizing.dragged(baseline,delta:delta,mode:.height),CGSize(width:370,height:400))
        XCTAssertEqual(NotchSizing.dragged(baseline,delta:delta,mode:.both),CGSize(width:470,height:400))
    }
    func testUniformScalingDoesNotStretchForIndependentDimensions() {
        let both=NotchSizing.layout(content:CGSize(width:740,height:640),baseline:baseline)
        XCTAssertEqual(both.scale,2);XCTAssertEqual(both.logical,baseline)
        let wide=NotchSizing.layout(content:CGSize(width:740,height:320),baseline:baseline)
        XCTAssertEqual(wide.scale,1);XCTAssertEqual(wide.logical.width,740)
        let tall=NotchSizing.layout(content:CGSize(width:370,height:640),baseline:baseline)
        XCTAssertEqual(tall.scale,1);XCTAssertEqual(tall.logical.height,640)
    }
    func testNativeAndLayerCoordinatesAgreeAtEverySize() async {
        await MainActor.run {
            let parent=NSView(frame:CGRect(x:0,y:0,width:1600,height:1000))
            let container=NSView();parent.addSubview(container)
            for scale:CGFloat in [1,1.5,2,2.5] {
                container.frame=CGRect(x:80,y:16,width:370*scale,height:280*scale)
                container.bounds=CGRect(x:0,y:0,width:370,height:280)
                let point=container.convert(CGPoint(x:80+100*scale,y:16+50*scale),from:parent)
                XCTAssertEqual(point.x,100,accuracy:0.001);XCTAssertEqual(point.y,50,accuracy:0.001)
            }
        }
    }
    func testComposerAndConversationStayInsideResponsiveLayout() async {
        await MainActor.run {
            let ui=MiniAppInterface()
            for size in [CGSize(width:354,height:257),CGSize(width:700,height:257),CGSize(width:354,height:650),CGSize(width:600,height:500)] {
                var composer=CGRect.zero,conversation=CGRect.zero
                ui.composerFrameChanged={rect,_ in composer=rect};ui.conversationFrameChanged={conversation=$0}
                ui.layout(in:CGRect(origin:.zero,size:size))
                XCTAssertTrue(CGRect(origin:.zero,size:size).contains(composer))
                XCTAssertTrue(CGRect(origin:.zero,size:size).contains(conversation))
                XCTAssertGreaterThan(conversation.height,0)
            }
        }
    }
    func testRenderResponsiveControls() async throws {
        guard ProcessInfo.processInfo.environment["KAI_SIZE_PREVIEWS"] == "1" else{return}
        let obj:[String:Any]=["project":"/tmp","projects":[["path":"/tmp","name":"Kai"]],"chats":[["id":"chat","name":"Resize Kai","status":"idle"]],"threadId":"chat","standaloneChats":[],"standalone":false,"threadName":"Resize Kai","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[],"notice":"","attachments":[],"selectedModel":"astra","reasoningEffort":"high","models":[["model":"astra","displayName":"Astra","supportedReasoningEfforts":[["reasoningEffort":"high"]]]]]
        let state=try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:obj))
        try await MainActor.run {
            for (name,size) in [("default",CGSize(width:354,height:257)),("wide",CGSize(width:750,height:257)),("tall",CGSize(width:354,height:600)),("large",CGSize(width:708,height:514))] {
                let sizing=NotchSizing.layout(content:size,baseline:CGSize(width:354,height:257))
                let root=CALayer();root.frame=CGRect(origin:.zero,size:size);root.backgroundColor=NSColor.black.cgColor
                let ui=MiniAppInterface();ui.layout(in:CGRect(origin:.zero,size:sizing.logical));ui.setState(state);ui.setRenderingScale(2*sizing.scale);ui.setVisible(true,duration:0)
                ui.layer.anchorPoint = .zero;ui.layer.position = .zero;ui.layer.setAffineTransform(CGAffineTransform(scaleX:sizing.scale,y:sizing.scale));root.addSublayer(ui.layer)
                let context=CGContext(data:nil,width:Int(size.width*2),height:Int(size.height*2),bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
                context.scaleBy(x:2,y:2);root.render(in:context)
                let data=NSBitmapImageRep(cgImage:context.makeImage()!).representation(using:.png,properties:[:])!
                try data.write(to:URL(fileURLWithPath:"/tmp/kai-size-\(name).png"))
            }
        }
    }

}
