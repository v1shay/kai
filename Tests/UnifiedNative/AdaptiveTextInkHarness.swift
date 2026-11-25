import AppKit
@main struct AdaptiveTextInkAudit {
 @MainActor static func main(){
  _=NSApplication.shared
  let host=NSView(frame:CGRect(x:0,y:0,width:600,height:400));host.wantsLayer=true;host.layer!.backgroundColor=NSColor(srgbRed:0.2,green:0.3,blue:0.4,alpha:0.6).cgColor
  let field=NSTextField(labelWithString:"Only text adapts");field.textColor=NSColor.white.withAlphaComponent(0.8);host.addSubview(field)
  let text=NSTextView(frame:CGRect(x:0,y:0,width:500,height:200));host.addSubview(text);let font=NSFont.systemFont(ofSize:13)
  text.textStorage!.setAttributedString(NSAttributedString(string:"Keep 🦀 selection and fonts",attributes:[.font:font,.foregroundColor:NSColor.white]));text.setSelectedRange(NSRange(location:5,length:2))
  let label=CATextLayer();label.string="Model";label.foregroundColor=NSColor.white.cgColor;label.backgroundColor=NSColor.red.withAlphaComponent(0.2).cgColor;label.opacity=0.7;host.layer!.addSublayer(label)
  let shape=CAShapeLayer();shape.strokeColor=NSColor.green.cgColor;shape.fillColor=NSColor.blue.cgColor;host.layer!.addSublayer(shape)
  let mask=CATextLayer();mask.foregroundColor=NSColor.white.cgColor;host.layer!.mask=mask
  let context=CGContext(data:nil,width:12,height:12,bitsPerComponent:8,bytesPerRow:48,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!;context.setFillColor(NSColor.white.cgColor);context.fill(CGRect(x:0,y:0,width:12,height:12));let image=context.makeImage()!;let pet=CALayer();pet.contents=image;pet.contentsRect=CGRect(x:0.25,y:0.5,width:0.25,height:0.25);host.layer!.addSublayer(pet)
  let originalBackground=host.layer!.backgroundColor,originalLabelBackground=label.backgroundColor,originalAppearance=host.appearance,originalSelection=text.selectedRange(),originalStroke=shape.strokeColor,originalFill=shape.fillColor,originalCaret=text.insertionPointColor,originalFrame=host.frame
  let ink=AdaptiveTextInk(host:host)
  func pump(_ duration:Double){let end=Date().addingTimeInterval(duration);while Date()<end{RunLoop.main.run(until:Date().addingTimeInterval(0.005))}}
  ink.apply(color:0x101318,animated:true);let start=field.textColor!.usingColorSpace(.sRGB)!.redComponent;pump(0.12);let middle=field.textColor!.usingColorSpace(.sRGB)!.redComponent;pump(0.22);let finish=field.textColor!.usingColorSpace(.sRGB)!.redComponent
  precondition(start>middle && middle>finish && finish<0.1,"Text color did not ease monotonically")
  precondition(abs(field.textColor!.alphaComponent-0.8)<0.001,"Text alpha changed")
  for _ in 0..<20{ink.refresh()}
  precondition(host.layer!.backgroundColor==originalBackground && label.backgroundColor==originalLabelBackground,"Text animation changed a background")
  precondition(shape.strokeColor==originalStroke && shape.fillColor==originalFill && label.opacity==0.7,"Text animation changed non-text primitives")
  precondition(host.appearance==originalAppearance && host.frame==originalFrame && host.layer!.mask === mask && mask.foregroundColor==NSColor.white.cgColor)
  precondition(text.string=="Keep 🦀 selection and fonts" && text.selectedRange()==originalSelection)
  precondition(text.textStorage!.attribute(.font,at:0,effectiveRange:nil) as? NSFont==font)
  precondition(AdaptiveColorMath.cgImage(pet.contents) === image && pet.contentsRect == CGRect(x:0.25,y:0.5,width:0.25,height:0.25),"Image or sprite atlas changed")
  ink.restore();precondition(field.textColor==NSColor.white.withAlphaComponent(0.8));precondition(label.foregroundColor==NSColor.white.cgColor);precondition(text.insertionPointColor==originalCaret)
  precondition(text.textStorage!.attribute(.foregroundColor,at:0,effectiveRange:nil) as? NSColor == .white)
  precondition(host.layer!.backgroundColor==originalBackground && shape.strokeColor==originalStroke)
  print("PASS: text-only eased animation; glass/background/stroke/fill/opacity/appearance/mask/frame unchanged; selection, fonts, alpha and original text restored")
 }
}
