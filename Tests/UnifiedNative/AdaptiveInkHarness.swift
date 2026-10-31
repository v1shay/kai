import AppKit

/// No capture permission, real provider, user window, or screenshot is needed.
@main struct AdaptiveInkAudit {
    @MainActor static func main() throws {
        _=NSApplication.shared
        let host=NSView(frame:CGRect(x:0,y:0,width:600,height:400));host.wantsLayer=true
        let field=NSTextField(labelWithString:"Read this panel");field.textColor = .white;host.addSubview(field)
        let text=NSTextView(frame:CGRect(x:0,y:0,width:500,height:200));host.addSubview(text)
        let font=NSFont.systemFont(ofSize:13)
        text.textStorage!.setAttributedString(NSAttributedString(string:"Keep 🦀 selection and syntax",attributes:[.font:font,.foregroundColor:NSColor.white]))
        text.setSelectedRange(NSRange(location:5,length:2));let selection=text.selectedRange();let caret=text.insertionPointColor
        let label=CATextLayer();label.string="Model";label.foregroundColor=NSColor.white.cgColor;label.frame=CGRect(x:10,y:10,width:80,height:20);host.layer!.addSublayer(label)
        let stroke=CAShapeLayer();stroke.strokeColor=NSColor.white.cgColor;host.layer!.addSublayer(stroke)
        let mask=CATextLayer();mask.foregroundColor=NSColor.white.cgColor;let parent=CALayer();parent.mask=mask;host.layer!.addSublayer(parent)
        func image(_ color:NSColor)->CGImage{let context=CGContext(data:nil,width:12,height:12,bitsPerComponent:8,bytesPerRow:48,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue|CGBitmapInfo.byteOrder32Big.rawValue)!;context.setFillColor(color.usingColorSpace(.sRGB)!.cgColor);context.fill(CGRect(x:0,y:0,width:12,height:12));return context.makeImage()!}
        let white=image(.white),black=image(.black),pet=CALayer(),icon=CALayer(),changed=CALayer()
        pet.name="pet-atlas";pet.contents=white;pet.contentsRect=CGRect(x:0,y:0,width:0.5,height:0.5)
        icon.contents=white;changed.contents=white;[pet,icon,changed].forEach{host.layer!.addSublayer($0)}
        let ink=AdaptiveNativeInk(host:host);let originalAppearance=host.appearance;let originalStroke=stroke.strokeColor
        ink.apply(ink:"dark",backgrounds:[[1,1,1]],target:4.5)
        precondition(field.textColor!.usingColorSpace(.sRGB)!.redComponent<0.1,"Bright panel did not use black ink")
        precondition(text.string=="Keep 🦀 selection and syntax" && text.selectedRange()==selection)
        precondition(text.textStorage!.attribute(.font,at:0,effectiveRange:nil) as? NSFont==font)
        precondition(AdaptiveColorMath.cgImage(pet.contents) === white,"Sprite pixels or atlas coordinates changed")
        precondition(mask.foregroundColor==NSColor.white.cgColor,"Animation mask recolored")
        precondition(AdaptiveColorMath.cgImage(icon.contents) !== white,"Template control stayed white")
        changed.contents=black
        // Repeated updates must preserve original values and remain bounded.
        let at=Date();for _ in 0..<100{ink.apply(ink:"dark",backgrounds:[[1,1,1]],target:4.5)}
        let elapsed=Date().timeIntervalSince(at);precondition(elapsed<2,"Native palette application exceeded validation budget")
        ink.restore()
        precondition(field.textColor == .white && stroke.strokeColor==originalStroke)
        precondition(host.appearance==originalAppearance && text.insertionPointColor==caret)
        precondition(AdaptiveColorMath.cgImage(icon.contents) === white)
        precondition(AdaptiveColorMath.cgImage(changed.contents) === black,"Restoration overwrote a newly changed icon")
        precondition(text.textStorage!.attribute(.foregroundColor,at:0,effectiveRange:nil) as? NSColor == .white)
        precondition(text.textStorage!.attribute(NSAttributedString.Key("kaiAdaptiveOriginalInk"),at:0,effectiveRange:nil)==nil)
        precondition(text.string=="Keep 🦀 selection and syntax" && text.selectedRange()==selection)
        let layer=CAGradientLayer();layer.frame=CGRect(x:0,y:0,width:100,height:100);layer.colors=[NSColor.black.cgColor,NSColor.white.cgColor];layer.locations=[0,1]
        let middle=AdaptiveColorMath.gradient(layer,at:CGPoint(x:50,y:50))!;precondition(abs(middle[0]-0.5)<0.001)
        precondition(abs(AdaptiveColorMath.luminance([1,1,1])-1)<0.00001)
        precondition(abs(AdaptiveColorMath.contrast(0,1)-21)<0.00001)
        let screen=CGRect(x:-2000,y:-900,width:2000,height:1200)
        let bgra:[UInt8]=[0,0,255,255,0,255,0,255,255,0,0,255,255,255,255,255]
        precondition(AdaptiveColorMath.sampleBGRA(bgra,width:2,height:2,screen:screen,point:CGPoint(x:-1500,y:0))! == [1,0,0])
        precondition(AdaptiveColorMath.sampleBGRA(bgra,width:2,height:2,screen:screen,point:CGPoint(x:-500,y:-600))! == [1,1,1])
        precondition(AdaptiveColorMath.sampleBGRA([],width:2,height:2,screen:screen,point:.zero)==nil)
        precondition(AdaptiveColorMath.sampleBGRA(bgra,width:2,height:2,screen:screen,point:CGPoint(x:500,y:500))==nil)
        print("PASS: adaptive native ink, selection/fonts/strings, mask and sprite isolation, source-swap restoration, appearance/caret restoration, gradient math, BGRA/negative-origin/Retina-independent display sampling; 100 updates \(elapsed)s")
    }
}
