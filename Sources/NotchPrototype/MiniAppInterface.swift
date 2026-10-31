import AppKit
import CoreText
import ImageIO
import QuickLookThumbnailing

/// Core Text centers the actual typographic line, rather than CATextLayer's
/// top-aligned default, inside the footer strip.
private final class CenteredComposerTextLayer:CATextLayer {
    override func draw(in context:CGContext){
        guard let text=string as? String,!text.isEmpty else{return}
        let face=(font as? NSFont) ?? AppTypography.ui(ofSize:fontSize)
        let attributes:[NSAttributedString.Key:Any]=[.font:face,.foregroundColor:NSColor(cgColor:foregroundColor ?? NSColor.white.cgColor) ?? .white]
        var line=CTLineCreateWithAttributedString(NSAttributedString(string:text,attributes:attributes))
        if CTLineGetTypographicBounds(line,nil,nil,nil)>Double(bounds.width){let token=CTLineCreateWithAttributedString(NSAttributedString(string:"…",attributes:attributes));line=CTLineCreateTruncatedLine(line,Double(bounds.width),truncationMode == .middle ? .middle:truncationMode == .start ? .start:.end,token) ?? line}
        var ascent:CGFloat=0,descent:CGFloat=0,leading:CGFloat=0;_ = CTLineGetTypographicBounds(line,&ascent,&descent,&leading)
        let ink=CTLineGetBoundsWithOptions(line,.useGlyphPathBounds);let baseline=ink.isEmpty ? (bounds.height-ascent-descent)/2+descent:(bounds.height-ink.height)/2-ink.minY
        context.saveGState();context.textMatrix = .identity;context.textPosition=CGPoint(x:0,y:baseline);CTLineDraw(line,context);context.restoreGState()
    }
}

private final class FileTypeGlyphLayer:CALayer {
    private enum Kind { case folder, python, html, css, swift, code(String), document, data, image, video, audio, archive, table, slides, font, database, config, git, package, lock }
    private let gradient=CAGradientLayer(),maskGroup=CALayer(),outline=CAShapeLayer(),mark=CAShapeLayer()
    private let kind:Kind,open:Bool
    init(_ url:URL,isDirectory:Bool,open:Bool) {
        self.open=open;kind=Self.kind(url,isDirectory);super.init();contentsScale=NSScreen.main?.backingScaleFactor ?? 2
        gradient.startPoint=CGPoint(x:0,y:0.2);gradient.endPoint=CGPoint(x:1,y:0.8);gradient.locations=[0,0.52,1];gradient.mask=maskGroup;addSublayer(gradient)
        for shape in [outline,mark] {shape.fillColor=nil;shape.strokeColor=NSColor.white.cgColor;shape.lineCap = .round;shape.lineJoin = .round;maskGroup.addSublayer(shape)}
    }
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){let source=layer as? FileTypeGlyphLayer;kind=source?.kind ?? .document;open=source?.open ?? false;super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();gradient.frame=bounds;maskGroup.frame=bounds;outline.frame=bounds;mark.frame=bounds;outline.lineWidth=0.9;mark.lineWidth=0.82;mark.fillColor=nil;mark.strokeColor=NSColor.white.cgColor;mark.fillRule = .evenOdd;outline.path=basePath();mark.path=markPath()}
    func setColors(_ target:[CGColor],animated:Bool){
        let colors=target.count == 3 ? target:[NSColor.systemBlue.cgColor,NSColor.systemPurple.cgColor,NSColor.white.cgColor],from=(gradient.presentation()?.colors as? [CGColor]) ?? (gradient.colors as? [CGColor]) ?? colors
        CATransaction.begin();CATransaction.setDisableActions(true);gradient.colors=colors;CATransaction.commit()
        guard animated else{return};let animation=CAKeyframeAnimation(keyPath:"colors");animation.values=(0..<18).map{step in zip(from,colors).map{Self.mix($0.0,$0.1,CGFloat(step)/17)}};animation.duration=0.72;animation.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);gradient.add(animation,forKey:"petGradient")
    }
    private func basePath()->CGPath {
        let p=CGMutablePath()
        if case .folder=kind {
            p.move(to:CGPoint(x:4.6,y:3.2));p.addLine(to:CGPoint(x:8.1,y:3.2));p.addLine(to:CGPoint(x:9.5,y:4.6));p.addLine(to:CGPoint(x:14.2,y:4.6))
            if open {p.addLine(to:CGPoint(x:13.1,y:10.8));p.addLine(to:CGPoint(x:4.7,y:10.8));p.closeSubpath();p.move(to:CGPoint(x:5.1,y:9.1));p.addLine(to:CGPoint(x:12.8,y:9.1));p.addLine(to:CGPoint(x:14,y:6.1))}
            else {p.addLine(to:CGPoint(x:14.2,y:10.8));p.addLine(to:CGPoint(x:4.6,y:10.8));p.closeSubpath()}
        } else if case .document=kind {
            p.move(to:CGPoint(x:4.7,y:1.6));p.addLine(to:CGPoint(x:10.8,y:1.6));p.addLine(to:CGPoint(x:13.8,y:4.6));p.addLine(to:CGPoint(x:13.8,y:11.5));p.addLine(to:CGPoint(x:4.7,y:11.5));p.closeSubpath();p.move(to:CGPoint(x:10.8,y:1.8));p.addLine(to:CGPoint(x:10.8,y:4.7));p.addLine(to:CGPoint(x:13.6,y:4.7))
        }
        return p
    }
    private func markPath()->CGPath? {
        if case .folder=kind {let p=CGMutablePath();p.move(to:CGPoint(x:1.2,y:5.1));p.addLine(to:CGPoint(x:2.7,y:6.6));p.addLine(to:CGPoint(x:1.2,y:8.1));return p}
        switch kind {
        case .python:
            mark.fillColor=NSColor.white.cgColor;mark.strokeColor=nil
            let p=CGMutablePath();p.move(to:CGPoint(x:3,y:6.2));p.addCurve(to:CGPoint(x:6.2,y:2),control1:CGPoint(x:3,y:3.7),control2:CGPoint(x:4.5,y:2));p.addLine(to:CGPoint(x:8.7,y:2));p.addCurve(to:CGPoint(x:10.9,y:4.2),control1:CGPoint(x:10.1,y:2),control2:CGPoint(x:10.9,y:2.8));p.addLine(to:CGPoint(x:10.9,y:5.5));p.addLine(to:CGPoint(x:6.3,y:5.5));p.addCurve(to:CGPoint(x:5.1,y:6.7),control1:CGPoint(x:5.6,y:5.5),control2:CGPoint(x:5.1,y:6));p.addLine(to:CGPoint(x:5.1,y:7.5));p.addLine(to:CGPoint(x:3,y:7.5));p.closeSubpath();p.move(to:CGPoint(x:12.6,y:6));p.addLine(to:CGPoint(x:10.5,y:6));p.addLine(to:CGPoint(x:10.5,y:6.8));p.addCurve(to:CGPoint(x:9.3,y:8),control1:CGPoint(x:10.5,y:7.5),control2:CGPoint(x:10,y:8));p.addLine(to:CGPoint(x:4.7,y:8));p.addLine(to:CGPoint(x:4.7,y:9.3));p.addCurve(to:CGPoint(x:6.9,y:11.5),control1:CGPoint(x:4.7,y:10.7),control2:CGPoint(x:5.5,y:11.5));p.addLine(to:CGPoint(x:9.4,y:11.5));p.addCurve(to:CGPoint(x:12.6,y:7.3),control1:CGPoint(x:11.1,y:11.5),control2:CGPoint(x:12.6,y:10));p.closeSubpath();p.addEllipse(in:CGRect(x:6.8,y:3.25,width:0.75,height:0.75));p.addEllipse(in:CGRect(x:8.7,y:9.45,width:0.75,height:0.75));return p
        case .html:return shield("5")
        case .css:return shield("3")
        case .swift:
            mark.fillColor=NSColor.white.cgColor;mark.strokeColor=nil
            let p=CGMutablePath();p.move(to:CGPoint(x:2.1,y:3));p.addCurve(to:CGPoint(x:8.8,y:7.3),control1:CGPoint(x:4.2,y:5.1),control2:CGPoint(x:6.1,y:6.4));p.addCurve(to:CGPoint(x:5.1,y:2.5),control1:CGPoint(x:7.1,y:5.1),control2:CGPoint(x:6.1,y:3.7));p.addCurve(to:CGPoint(x:10.7,y:6.6),control1:CGPoint(x:7.3,y:4),control2:CGPoint(x:9.1,y:5.3));p.addCurve(to:CGPoint(x:8.4,y:2),control1:CGPoint(x:9.9,y:4.5),control2:CGPoint(x:9.1,y:3));p.addCurve(to:CGPoint(x:13.1,y:9.7),control1:CGPoint(x:12.2,y:4.5),control2:CGPoint(x:14.2,y:7.4));p.addCurve(to:CGPoint(x:10.7,y:11.3),control1:CGPoint(x:12.5,y:10.9),control2:CGPoint(x:11.4,y:11.4));p.addCurve(to:CGPoint(x:7.9,y:9.4),control1:CGPoint(x:10.1,y:10.3),control2:CGPoint(x:9.2,y:9.5));p.addCurve(to:CGPoint(x:2.1,y:3),control1:CGPoint(x:5.9,y:8.3),control2:CGPoint(x:3.8,y:6.1));p.closeSubpath();return p
        case .code(let text):mark.fillColor=NSColor.white.cgColor;mark.strokeColor=nil;return Self.textPath(text,in:CGRect(x:1.4,y:3,width:13.2,height:8.4))
        case .document:let p=CGMutablePath();for y in [6.3,8.2,10.1] as [CGFloat]{p.move(to:CGPoint(x:6.2,y:y));p.addLine(to:CGPoint(x:12.2,y:y))};return p
        case .data:let p=CGMutablePath();p.move(to:CGPoint(x:7.2,y:5.9));p.addLine(to:CGPoint(x:6.1,y:7.7));p.addLine(to:CGPoint(x:7.2,y:9.5));p.move(to:CGPoint(x:11.3,y:5.9));p.addLine(to:CGPoint(x:12.4,y:7.7));p.addLine(to:CGPoint(x:11.3,y:9.5));return p
        case .image:let p=CGMutablePath();p.addEllipse(in:CGRect(x:10.4,y:5.7,width:1.4,height:1.4));p.move(to:CGPoint(x:5.7,y:10.1));p.addLine(to:CGPoint(x:8.2,y:7.5));p.addLine(to:CGPoint(x:9.7,y:9));p.addLine(to:CGPoint(x:11,y:7.9));p.addLine(to:CGPoint(x:13,y:10.1));return p
        case .video:let p=CGMutablePath();p.move(to:CGPoint(x:7.2,y:5.6));p.addLine(to:CGPoint(x:11.8,y:7.8));p.addLine(to:CGPoint(x:7.2,y:10));p.closeSubpath();mark.fillColor=NSColor.white.cgColor;return p
        case .audio:let p=CGMutablePath();p.move(to:CGPoint(x:10.8,y:5.4));p.addLine(to:CGPoint(x:10.8,y:9.1));p.addEllipse(in:CGRect(x:7.8,y:8.5,width:3,height:2));p.move(to:CGPoint(x:10.8,y:5.6));p.addLine(to:CGPoint(x:12.5,y:5.2));return p
        case .archive:let p=CGMutablePath();for y in [5.6,7.2,8.8] as [CGFloat]{p.addRect(CGRect(x:8.6,y:y,width:1.2,height:0.75))};p.addRect(CGRect(x:7.7,y:10,width:3,height:0.9));return p
        case .table:let p=CGMutablePath();p.addRect(CGRect(x:6,y:5.6,width:6.5,height:4.8));p.move(to:CGPoint(x:8.2,y:5.7));p.addLine(to:CGPoint(x:8.2,y:10.3));p.move(to:CGPoint(x:10.3,y:5.7));p.addLine(to:CGPoint(x:10.3,y:10.3));p.move(to:CGPoint(x:6.1,y:7.9));p.addLine(to:CGPoint(x:12.4,y:7.9));return p
        case .slides:let p=CGMutablePath();p.addRect(CGRect(x:6,y:5.6,width:6.4,height:3.7));p.move(to:CGPoint(x:9.2,y:9.3));p.addLine(to:CGPoint(x:9.2,y:10.5));p.move(to:CGPoint(x:7.7,y:10.6));p.addLine(to:CGPoint(x:10.7,y:10.6));return p
        case .font:return Self.textPath("Aa",in:CGRect(x:5.3,y:5.2,width:7.9,height:5.4))
        case .database:let p=CGMutablePath();p.addEllipse(in:CGRect(x:6.1,y:5.1,width:6.2,height:2.2));p.move(to:CGPoint(x:6.1,y:6.2));p.addLine(to:CGPoint(x:6.1,y:9.5));p.addCurve(to:CGPoint(x:12.3,y:9.5),control1:CGPoint(x:6.6,y:11),control2:CGPoint(x:11.8,y:11));p.addLine(to:CGPoint(x:12.3,y:6.2));return p
        case .config:let p=CGMutablePath();for y in [6,8,10] as [CGFloat]{p.move(to:CGPoint(x:6,y:y));p.addLine(to:CGPoint(x:12.5,y:y))};p.addEllipse(in:CGRect(x:7,y:5.25,width:1.5,height:1.5));p.addEllipse(in:CGRect(x:10,y:7.25,width:1.5,height:1.5));p.addEllipse(in:CGRect(x:8,y:9.25,width:1.5,height:1.5));return p
        case .git:let p=CGMutablePath();p.move(to:CGPoint(x:7,y:5.7));p.addLine(to:CGPoint(x:7,y:9.6));p.addCurve(to:CGPoint(x:11.4,y:6.4),control1:CGPoint(x:10.7,y:9.6),control2:CGPoint(x:11.4,y:8.1));p.addEllipse(in:CGRect(x:6.3,y:5,width:1.4,height:1.4));p.addEllipse(in:CGRect(x:6.3,y:9,width:1.4,height:1.4));p.addEllipse(in:CGRect(x:10.7,y:5.7,width:1.4,height:1.4));return p
        case .package:let p=CGMutablePath();p.move(to:CGPoint(x:6.1,y:6.7));p.addLine(to:CGPoint(x:9.2,y:5.2));p.addLine(to:CGPoint(x:12.3,y:6.7));p.addLine(to:CGPoint(x:12.3,y:9.7));p.addLine(to:CGPoint(x:9.2,y:11));p.addLine(to:CGPoint(x:6.1,y:9.7));p.closeSubpath();p.move(to:CGPoint(x:6.3,y:6.8));p.addLine(to:CGPoint(x:9.2,y:8.2));p.addLine(to:CGPoint(x:12.1,y:6.8));p.move(to:CGPoint(x:9.2,y:8.2));p.addLine(to:CGPoint(x:9.2,y:10.8));return p
        case .lock:let p=CGMutablePath();p.addRoundedRect(in:CGRect(x:6.5,y:7,width:5.4,height:3.7),cornerWidth:0.7,cornerHeight:0.7);p.move(to:CGPoint(x:7.5,y:7));p.addCurve(to:CGPoint(x:10.9,y:7),control1:CGPoint(x:7.5,y:3.9),control2:CGPoint(x:10.9,y:3.9));return p
        case .folder:return nil
        }
    }
    private static func kind(_ url:URL,_ directory:Bool)->Kind {
        if directory{return .folder};let name=url.lastPathComponent.lowercased(),ext=url.pathExtension.lowercased()
        if [".gitignore",".gitattributes",".gitmodules"].contains(name){return .git}
        if ["dockerfile","makefile","podfile","gemfile"].contains(name){return .code(name == "dockerfile" ? "DK":"MK")}
        if ["package.json","package.swift","cargo.toml","go.mod","composer.json"].contains(name){return .package}
        if ["py","pyw","pyi"].contains(ext){return .python};if ["html","htm"].contains(ext){return .html};if ["css","scss","sass","less"].contains(ext){return .css};if ext == "swift"{return .swift}
        let codes:[String:String]=["js":"JS","mjs":"JS","cjs":"JS","jsx":"JS","ts":"TS","mts":"TS","cts":"TS","tsx":"TS","java":"JV","kt":"KT","kts":"KT","c":"C","h":"C","cc":"C+","cpp":"C+","cxx":"C+","hpp":"C+","cs":"C#","go":"GO","rs":"RS","rb":"RB","php":"PH","dart":"DT","scala":"SC","r":"R","lua":"LU","pl":"PL","m":"OC","mm":"OC","sh":">_","bash":">_","zsh":">_","fish":">_","vue":"VU","svelte":"SV"]
        if let code=codes[ext]{return .code(code)}
        if ["json","jsonl","geojson","xml","graphql"].contains(ext){return .data}
        if ["yaml","yml","toml","ini","conf","config","env","properties","plist"].contains(ext){return .config}
        if ["png","jpg","jpeg","heic","heif","webp","gif","svg","tif","tiff","bmp","ico","avif"].contains(ext){return .image}
        if ["mov","mp4","m4v","webm","mkv","avi"].contains(ext){return .video}
        if ["mp3","wav","m4a","aac","flac","ogg","aiff"].contains(ext){return .audio}
        if ["zip","tar","gz","bz2","xz","7z","rar"].contains(ext){return .archive}
        if ["xls","xlsx","csv","tsv","numbers"].contains(ext){return .table}
        if ["ppt","pptx","key"].contains(ext){return .slides}
        if ["ttf","otf","woff","woff2"].contains(ext){return .font}
        if ["sql","db","sqlite","sqlite3"].contains(ext){return .database}
        if ["pem","crt","cer","key","p12"].contains(ext){return .lock}
        if ext == "pdf" {return .code("PDF")};if ["md","mdx","rst"].contains(ext){return .code("MD")};return .document
    }
    private func shield(_ value:String)->CGPath {
        let p=CGMutablePath();p.move(to:CGPoint(x:3,y:2));p.addLine(to:CGPoint(x:13,y:2));p.addLine(to:CGPoint(x:12.1,y:10));p.addLine(to:CGPoint(x:8,y:12));p.addLine(to:CGPoint(x:3.9,y:10));p.closeSubpath();if let number=Self.textPath(value,in:CGRect(x:5.2,y:3.5,width:5.7,height:6.4)){p.addPath(number)};return p
    }
    private static func textPath(_ text:String,in rect:CGRect)->CGPath? {
        let path=CGMutablePath(),font=CTFontCreateWithName("SFMono-Bold" as CFString,text.count > 2 ? 4.2:6.2,nil),line=CTLineCreateWithAttributedString(NSAttributedString(string:text,attributes:[.font:font])),runs=CTLineGetGlyphRuns(line) as! [CTRun];var minX=CGFloat.greatestFiniteMagnitude,maxX:CGFloat=0,pieces=[(CGPath,CGPoint)]()
        for run in runs {let count=CTRunGetGlyphCount(run);for i in 0..<count {var glyph=CGGlyph(),position=CGPoint.zero;CTRunGetGlyphs(run,CFRange(location:i,length:1),&glyph);CTRunGetPositions(run,CFRange(location:i,length:1),&position);if let glyphPath=CTFontCreatePathForGlyph(font,glyph,nil){pieces.append((glyphPath,position));minX=min(minX,position.x+glyphPath.boundingBox.minX);maxX=max(maxX,position.x+glyphPath.boundingBox.maxX)}}}
        let width=max(1,maxX-minX),scale=min(1,rect.width/width),offset=CGPoint(x:rect.midX-width*scale/2-minX*scale,y:rect.minY)
        for (piece,position) in pieces {var transform=CGAffineTransform(a:scale,b:0,c:0,d:scale,tx:offset.x+position.x*scale,ty:offset.y);if let copy=piece.copy(using:&transform){path.addPath(copy)}};return path
    }
    private static func mix(_ a:CGColor,_ b:CGColor,_ t:CGFloat)->CGColor {let x=NSColor(cgColor:a)?.usingColorSpace(.sRGB) ?? .clear,y=NSColor(cgColor:b)?.usingColorSpace(.sRGB) ?? .clear;return NSColor(srgbRed:x.redComponent+(y.redComponent-x.redComponent)*t,green:x.greenComponent+(y.greenComponent-x.greenComponent)*t,blue:x.blueComponent+(y.blueComponent-x.blueComponent)*t,alpha:x.alphaComponent+(y.alphaComponent-x.alphaComponent)*t).cgColor}
}

private final class FileRowLayer:CALayer {
    let url:URL
    let isDirectory:Bool
    private let icon:FileTypeGlyphLayer,title=CATextLayer(),finder=CALayer(),clipboard=CALayer(),actionHover=CALayer()
    private var hoveredAction:String?
    var actionTargets:[(CGRect,String)]{[(CGRect(x:bounds.width-24,y:1,width:11,height:12),"reveal"),(CGRect(x:bounds.width-11,y:1,width:11,height:12),"copy")]}
    init(_ url:URL,isDirectory:Bool,open:Bool){self.url=url;self.isDirectory=isDirectory;icon=FileTypeGlyphLayer(url,isDirectory:isDirectory,open:open);super.init();title.string=url.lastPathComponent;title.font=AppTypography.ui(ofSize:8);title.fontSize=8;title.foregroundColor=NSColor.white.withAlphaComponent(0.52).cgColor;title.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;title.truncationMode = .end;addSublayer(icon);addSublayer(title);actionHover.opacity=0;addSublayer(actionHover);for (item,action) in [(finder,"reveal"),(clipboard,"copy")]{item.contents=FileActionSymbols.image(action,url:url);item.contentsGravity = .resizeAspect;item.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;addSublayer(item)}}
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){let source=layer as? FileRowLayer;url=source?.url ?? URL(fileURLWithPath:"/");isDirectory=source?.isDirectory ?? false;icon=FileTypeGlyphLayer(url,isDirectory:isDirectory,open:false);super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();icon.frame=CGRect(x:0,y:0.5,width:15.5,height:13);title.frame=CGRect(x:18,y:2,width:max(0,bounds.width-46),height:11);finder.frame=actionTargets[0].0;clipboard.frame=actionTargets[1].0;FileActionSymbols.updateHover(hoveredAction,targets:actionTargets,backdrop:actionHover,animated:false)}
    func setActionHover(_ action:String?){
        guard hoveredAction != action else{return};hoveredAction=action
        FileActionSymbols.updateHover(action,targets:actionTargets,backdrop:actionHover)
    }
    func refreshAction(_ action:String){(action == "reveal" ? finder:clipboard).contents=FileActionSymbols.image(action,url:url)}
    var string:String {get{title.string as? String ?? ""}set{title.string=newValue}}
    func setColors(_ colors:[CGColor],animated:Bool){icon.setColors(colors,animated:animated)}
}

private final class FilePreviewLayer:CALayer {
    private let title=CATextLayer(),body=CATextLayer(),image=CALayer(),close=CALayer(),mark=CAShapeLayer(),finder=CALayer(),clipboard=CALayer(),actionHover=CALayer()
    private var hoveredAction:String?
    var actionTargets:[(CGRect,String)]{[(CGRect(x:bounds.width-41,y:bounds.height-14,width:11,height:10),"reveal"),(CGRect(x:bounds.width-28,y:bounds.height-14,width:11,height:10),"copy")]}
    private(set) var representedURL:URL?,textContent:String?
    var onContentChanged:(()->Void)?
    private var loadID=UUID()
    override init(){super.init();opacity=0;masksToBounds=true;title.font=AppTypography.ui(ofSize:7.5,weight:.semibold);title.fontSize=7.5;title.foregroundColor=NSColor.white.withAlphaComponent(0.8).cgColor;title.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;title.truncationMode = .middle;addSublayer(title);body.font=AppTypography.code(ofSize:5.7,weight:.regular);body.fontSize=5.7;body.foregroundColor=NSColor.white.withAlphaComponent(0.48).cgColor;body.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;body.isWrapped=true;body.truncationMode = .end;addSublayer(body);image.contentsGravity = .resizeAspect;image.cornerRadius=5;image.masksToBounds=true;addSublayer(image);close.backgroundColor=NSColor.white.withAlphaComponent(0.09).cgColor;close.cornerRadius=5;addSublayer(close);actionHover.opacity=0;addSublayer(actionHover);for (item,action) in [(finder,"reveal"),(clipboard,"copy")]{item.contents=FileActionSymbols.image(action);item.contentsGravity = .resizeAspect;item.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;addSublayer(item)};mark.fillColor=nil;mark.strokeColor=NSColor.white.withAlphaComponent(0.72).cgColor;mark.lineWidth=0.9;mark.lineCap = .round;close.addSublayer(mark)}
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();title.frame=CGRect(x:3,y:bounds.height-14,width:max(0,bounds.width-48),height:10);finder.frame=actionTargets[0].0;clipboard.frame=actionTargets[1].0;FileActionSymbols.updateHover(hoveredAction,targets:actionTargets,backdrop:actionHover,animated:false);close.frame=CGRect(x:bounds.width-13,y:bounds.height-14,width:10,height:10);mark.frame=close.bounds;let p=CGMutablePath();p.move(to:CGPoint(x:3,y:3));p.addLine(to:CGPoint(x:7,y:7));p.move(to:CGPoint(x:7,y:3));p.addLine(to:CGPoint(x:3,y:7));mark.path=p;body.frame=CGRect(x:3,y:2,width:bounds.width-6,height:max(0,bounds.height-19));image.frame=CGRect(x:3,y:3,width:bounds.width-6,height:max(0,bounds.height-21))}
    var closeTarget:CGRect{close.frame}
    func closeContains(_ point:CGPoint)->Bool{close.frame.insetBy(dx:-4,dy:-4).contains(point)}
    func show(_ url:URL,onReady:@escaping ()->Void){
        representedURL=url;loadID=UUID();let token=loadID;refreshAction("reveal");refreshAction("copy")
        title.string=url.lastPathComponent;image.contents=nil;image.opacity=0;body.string="loading…";textContent="loading…";notifyContent()
        let textExtensions=Set(["swift","md","txt","json","plist","py","js","ts","tsx","css","html","yaml","yml","sh","toml","csv"])
        DispatchQueue.global(qos:.userInitiated).async { [weak self] in
            if textExtensions.contains(url.pathExtension.lowercased()) {
                let result=Result {String(decoding:try Self.previewPrefix(url),as:UTF8.self)}
                DispatchQueue.main.async {guard let self,self.loadID==token else{return};switch result{case .success(let text):self.body.string=text;self.textContent=text;self.notifyContent();case .failure(let error):self.failure(error)};onReady()}
                return
            }
            if let source=CGImageSourceCreateWithURL(url as CFURL,nil),let picture=CGImageSourceCreateThumbnailAtIndex(source,0,[kCGImageSourceCreateThumbnailFromImageAlways:true,kCGImageSourceThumbnailMaxPixelSize:720,kCGImageSourceCreateThumbnailWithTransform:true] as CFDictionary){
                DispatchQueue.main.async {guard let self,self.loadID==token else{return};self.textContent=nil;self.notifyContent();self.body.opacity=0;self.image.contents=picture;self.image.opacity=1;onReady()}
                return
            }
            let request=QLThumbnailGenerator.Request(fileAt:url,size:CGSize(width:180,height:180),scale:NSScreen.main?.backingScaleFactor ?? 2,representationTypes:.all)
            QLThumbnailGenerator.shared.generateBestRepresentation(for:request){result,error in DispatchQueue.main.async {guard let self,self.loadID==token else{return};if let result{self.textContent=nil;self.notifyContent();self.body.opacity=0;self.image.contents=result.nsImage;self.image.opacity=1}else{self.failure(error)};onReady()}}
        }
    }
    func setActionHover(_ action:String?){
        guard hoveredAction != action else{return};hoveredAction=action
        FileActionSymbols.updateHover(action,targets:actionTargets,backdrop:actionHover)
    }
    func refreshAction(_ action:String){(action == "reveal" ? finder:clipboard).contents=FileActionSymbols.image(action,url:representedURL)}
    private static func previewPrefix(_ url:URL)throws->Data {
        let handle=try FileHandle(forReadingFrom:url);defer{try? handle.close()}
        let limit=2*1024*1024,data=try handle.read(upToCount:limit+1) ?? Data()
        return data.count>limit ? Data(data.prefix(limit))+Data("\n\n[Preview limited to 2 MB. Use Show in Finder to open the full file.]".utf8):data
    }
    private func notifyContent(){body.opacity=onContentChanged == nil ? 1:0;onContentChanged?()}
    private func failure(_ error:Error?){body.opacity=1;image.opacity=0;body.string=error == nil ? "preview unavailable":"unable to preview\n\(error!.localizedDescription)";textContent=body.string as? String;notifyContent()}
}

private final class ChatRowLayer:CALayer {
    private let provider=CALayer(),statusHost=CALayer(),hide=CATextLayer(),title=CATextLayer()
    private let pet=CALayer()
    private var hasPet=false
    private let value:String,font=AppTypography.ui(ofSize:8)
    private var hovered=false,selected=false,running=false,lastRun:String?,shownSuccess:String?
    private var knownSuccesses=[String]()
    private var statusEngine:NotchIndicatorEngine?,profile:PetGradientProfile?,settle:DispatchWorkItem?
    init(_ value:String){
        self.value=value;super.init();masksToBounds=true
        title.string=value;title.font=font;title.fontSize=8;title.foregroundColor=NSColor.white.withAlphaComponent(0.42).cgColor;title.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;title.truncationMode = .end;addSublayer(title)
        hide.string="×";hide.font=font;hide.fontSize=10;hide.foregroundColor=NSColor.white.withAlphaComponent(0.65).cgColor;hide.backgroundColor=NSColor.black.cgColor;hide.alignmentMode = .center;hide.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;hide.opacity=0;hide.zPosition=1
        provider.opacity=0.65;statusHost.opacity=0;addSublayer(provider);addSublayer(pet);addSublayer(statusHost);addSublayer(hide);pet.isHidden=true
    }
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){let source=layer as? ChatRowLayer;value=source?.value ?? "";super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();CATransaction.begin();CATransaction.setDisableActions(true)
        provider.frame=CGRect(x:0,y:0,width:11,height:11);pet.frame=CGRect(x:0,y:-1,width:14,height:14);statusHost.frame=hasPet ? CGRect(x:12,y:1,width:8,height:8):provider.frame
        if hasPet{statusHost.setAffineTransform(CGAffineTransform(scaleX:0.72,y:0.72))}else{statusHost.setAffineTransform(.identity)}
        title.frame=CGRect(x:hasPet ? 24:19,y:1,width:max(0,bounds.width-(hasPet ? 37:32)),height:10);hide.frame=CGRect(x:bounds.width-12,y:0,width:12,height:12);CATransaction.commit()
    }
    func setPet(_ visual:InlineTaskPet?){guard hasPet != (visual != nil) || pet.name != visual?.key else{return};hasPet=visual != nil;pet.isHidden = !hasPet;provider.isHidden=hasPet;if let visual{visual.apply(to:pet)}else{pet.name=nil;pet.contents=nil;pet.removeAllAnimations()};setNeedsLayout()}
    func setHovered(_ active:Bool){
        guard active != hovered else{return};hovered=active;hide.opacity=active ? 1:0;title.removeAnimation(forKey:"marquee")
        let available=max(0,bounds.width-32),full=ceil((value as NSString).size(withAttributes:[.font:font]).width)
        CATransaction.begin();CATransaction.setDisableActions(true);title.setAffineTransform(.identity);title.truncationMode=active ? .none:.end;title.frame=CGRect(x:19,y:1,width:active ? max(available,full):available,height:10);CATransaction.commit()
        guard active,full>available else{return};let distance=full-available+3,a=CAKeyframeAnimation(keyPath:"transform.translation.x");a.values=[0,0,-distance,-distance,0];a.keyTimes=[0,0.12,0.48,0.72,1];a.duration=max(2.4,Double(distance)/11);a.repeatCount = .infinity;a.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);title.add(a,forKey:"marquee")
    }
    func setSelected(_ selected:Bool){guard selected != self.selected else{return};self.selected=selected;title.foregroundColor=NSColor.white.withAlphaComponent(selected ? 0.9:0.42).cgColor}
    @MainActor func setProvider(_ id:String?,profile:PetGradientProfile?){
        self.profile=profile;provider.contents=ProviderIcons.image(id ?? "codex")?.cgImage(forProposedRect:nil,context:nil,hints:nil);provider.contentsGravity = .resizeAspect
        if let profile{statusEngine?.setProfile(profile,wave:false)}
    }
    private func fade(_ layer:CALayer,to value:Float){
        let animation=CABasicAnimation(keyPath:"opacity");animation.fromValue=layer.presentation()?.opacity ?? layer.opacity;animation.toValue=value;animation.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:0.24;animation.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.84,0.2,1)
        CATransaction.begin();CATransaction.setDisableActions(true);layer.opacity=value;CATransaction.commit();layer.add(animation,forKey:"statusTransition")
    }
    @MainActor func setRunning(_ running:Bool,runID:String?,successfulRuns:[String]){
        if running{lastRun=runID}
        let success = !running ? successfulRuns.last(where:{!knownSuccesses.contains($0)}):nil
        for run in successfulRuns where !knownSuccesses.contains(run){knownSuccesses.append(run)}
        if knownSuccesses.count>128{knownSuccesses.removeFirst(knownSuccesses.count-128)}
        guard running != self.running || (success != nil && success != shownSuccess) else{return};self.running=running;settle?.cancel();settle=nil
        if statusEngine == nil{
            let engine=NotchIndicatorEngine();engine.setAccentVisibility(left:false,right:false)
            if let profile{engine.setProfile(profile,wave:false)}
            engine.layer.position=CGPoint(x:5.5,y:5.5);engine.layer.setAffineTransform(CGAffineTransform(scaleX:11/28,y:11/28));statusHost.addSublayer(engine.layer);statusEngine=engine
        }
        fade(provider,to:0);fade(statusHost,to:1)
        if running{statusEngine?.showThinking()}
        else{
            guard let success else{statusEngine?.hide();fade(statusHost,to:0);fade(provider,to:0.65);return}
            shownSuccess=success;statusEngine?.showScene("success",looping:false)
            let work=DispatchWorkItem{[weak self] in guard let self,!self.running else{return};self.fade(self.statusHost,to:0);self.fade(self.provider,to:0.65);self.statusEngine?.hide()}
            settle=work;DispatchQueue.main.asyncAfter(deadline:.now()+1.05,execute:work)
        }
    }
}

private final class AttachmentLayer:CALayer {
    private let close=CALayer(),mark=CAShapeLayer()
    let image:NSImage
    init(_ image:NSImage){self.image=image;super.init();cornerRadius=7;masksToBounds=true;contentsGravity = .resizeAspectFill;contents=image;close.backgroundColor=NSColor.black.withAlphaComponent(0.72).cgColor;close.cornerRadius=6;close.opacity=0;addSublayer(close);mark.fillColor=nil;mark.strokeColor=NSColor.white.withAlphaComponent(0.94).cgColor;mark.lineWidth=1.15;mark.lineCap = .round;close.addSublayer(mark)}
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){image=(layer as? AttachmentLayer)?.image ?? NSImage();super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();close.frame=CGRect(x:bounds.width-14,y:bounds.height-14,width:12,height:12);mark.frame=close.bounds;let p=CGMutablePath();p.move(to:CGPoint(x:3.5,y:3.5));p.addLine(to:CGPoint(x:8.5,y:8.5));p.move(to:CGPoint(x:8.5,y:3.5));p.addLine(to:CGPoint(x:3.5,y:8.5));mark.path=p}
    func setHovered(_ hovered:Bool){let target:Float=hovered ? 1:0,a=CABasicAnimation(keyPath:"opacity");a.fromValue=close.presentation()?.opacity ?? close.opacity;a.toValue=target;a.duration=0.16;CATransaction.begin();CATransaction.setDisableActions(true);close.opacity=target;CATransaction.commit();close.add(a,forKey:"hover")}
    var closeTarget:CGRect{close.frame}
    func closeContains(_ point:CGPoint)->Bool{close.frame.insetBy(dx:-4,dy:-4).contains(point)}
}

@MainActor final class MiniAppInterface {
    private let contextRing=CAShapeLayer(),contextTrack=CAShapeLayer(),contextText=CATextLayer()
    private var contextHit=CGRect.zero
    let layer=CALayer()
    private var renderingScale:CGFloat=NSScreen.main?.backingScaleFactor ?? 2
    func setRenderingScale(_ scale:CGFloat){renderingScale=scale;NotchSizing.updateTextResolution(layer,scale:scale)}
    var onUsageReset:((String)->Void)?
    var onFast:(()->Void)?
    var onOrchestrator:(()->Void)?
    var onUsageRefresh:(()->Void)?
    private let composerPopover=ComposerGlassPopover()
    private let agentPicker=NativeAgentPicker()
    private let effortLabel=CenteredComposerTextLayer(),fastBolt=CALayer(),usageLabel=CenteredComposerTextLayer(),usageTrack=CALayer(),usageFill=CALayer()
    private var fastWasOn:Bool?
    private let effortShimmer=EffortTileLayer(),fastShimmer=EffortTileLayer()
    private let fastTint=CAGradientLayer(),orchestratorChip=CALayer(),orchestratorShimmer=EffortTileLayer(),orchestratorGlyph=CALayer()
    private var orchestratorWasOn:Bool?
    private var hoveringFast=false
    private var effortShimmerKey="",hoveringEffort=false
    var onSettings: ((String,String)->Void)?
    var onModel:((String)->Void)?
    var onPermissions:((String?,String?)->Void)?
    var onNavigate: ((String)->Void)?
    var onContextMode: ((Bool)->Void)?
    var onAgent:((String)->Void)?
    var onVoice:(()->Void)?
    var onFocusComposer:(()->Void)?
    var onPendingTimeout:((String)->Void)?
    private let permissionGlyph=CAGradientLayer(),permissionMask=CALayer(),modelChip=CALayer(),modelIcon=CALayer(),modelName=CenteredComposerTextLayer(),effortChip=CALayer(),effortFill=CAGradientLayer(),effortThumb=CALayer()
    private var composerControlKey=[String](),composerHits=[(CGRect,String)]()
    // All visible glyphs and optical text centers share this footer rail.
    private let composerControlsCenterY:CGFloat=14.5
    private var borderlessComposer:Bool{modelState?.borderlessComposer == true}
    private var usesUnifiedComposer:Bool{EmbeddedCodeRuntime.shared.active || modelState?.workspaceAccess != nil}
    private let voiceWaveform=CALayer()
    private let voiceButton=CATextLayer()
    private var voiceActive=false,voiceStyle=""
    func setVoiceActive(_ active:Bool){voiceActive=active;let style="\(usesUnifiedComposer)-\(active)";guard style != voiceStyle else{return};voiceStyle=style;voiceButton.string=usesUnifiedComposer ? "":(active ? "■":"◉");voiceButton.foregroundColor=(active ? NSColor.systemRed:NSColor.white.withAlphaComponent(0.7)).cgColor;voiceWaveform.isHidden = !usesUnifiedComposer;voiceWaveform.contents=NSImage(systemSymbolName:active ? "stop.fill":"waveform",accessibilityDescription:nil)?.withSymbolConfiguration(.init(paletteColors:[active ? .systemRed:.white]))}
    private var modelState: KaiState?
    private var lightweightContext=false
    private let settingsBar=CALayer()
    private var settingsLayoutKey:[String]=[]
    private var actionsLayoutKey:[String]=[]
    private var actionSymbols=[String:NotchIndicatorEngine](),hoveredAction:String?
    private var bottomInset:CGFloat=0
    private var taskPets=[String:InlineTaskPet]()
    func setTaskPets(_ pets:[String:InlineTaskPet]){taskPets=pets;for(row,id)in activeRows+chatRows+codeRows+standaloneRows{row.setPet(id.hasPrefix("child:") ? pets[id]:nil)}}
    var interactiveBounds:CGRect{layer.bounds.union(usageLabel.frame).union(usageTrack.frame)}
    private var settingsHits=[(CGRect,String)]()
    var composerFrameChanged:((CGRect,Bool)->Void)?
    var conversationFrameChanged:((CGRect)->Void)?
    var previewContentChanged:((CGRect,String?,URL?,Bool)->Void)?
    var onFileActions:((URL)->Void)?
    var onFileAction:((String,URL)->Void)?
    var onFilesVisibleChanged:((Bool)->Void)?
    var onProject:((String)->Void)?,onChat:((String)->Void)?,onNew:(()->Void)?,onNewInProject:(()->Void)?,onFolder:((String)->Void)?,onSend:(()->Void)?,onApprove:(()->Void)?,onReject:(()->Void)?,onApprovalStop:(()->Void)?,onReview:(()->Void)?,onInterrupt:(()->Void)?,onClose:(()->Void)?,onImagesChanged:(([NSImage])->Void)?
    private let left=CALayer(), projectToggle=CAShapeLayer(), preview=FilePreviewLayer(), main=CALayer(), right=CALayer(), input=CAGradientLayer(), imageStrip=CALayer(), command=CALayer(), send=CAGradientLayer(), sendShade=CAGradientLayer(), sendGlass=CAGradientLayer(), arrow=CAShapeLayer(), message=CATextLayer(), response=CAGradientLayer(), responseMask=CALayer(), typingDots=(0..<3).map{_ in CALayer()},pendingHighlight=CALayer(),pendingRibbon=CAGradientLayer()
    private let newRow=CATextLayer(), projectNewRow=CATextLayer(), standaloneTitle=CATextLayer(),codeTitle=CATextLayer(), activeTitle=CATextLayer(), actionBar=CALayer(), closeButton=CATextLayer()
    private var projectRows=[(CALayer,String)](),chatRows=[(ChatRowLayer,String)](),standaloneRows=[(ChatRowLayer,String)](),activeRows=[(ChatRowLayer,String)](),codeRows=[(ChatRowLayer,String)](),codeChats=[KaiChat](),projects=[KaiProject](),chats=[KaiChat](),standaloneChats=[KaiChat](),selectedProject="",selectedThread:String?,fileEntries=[KaiFile](),fileRows=[FileRowLayer](),pathHits=[(CGRect,URL)](),filesOpen=false,previewOpen=false,previewFromFiles=false,projectsCollapsed=false,projectIntrudesComposer=false,projectOffset:CGFloat=0,fileOffset:CGFloat=0,approvalNeeded=false,turnActive=false
    private var sidebarContentHeight:CGFloat=0
    private var elsewhere=[KaiActiveTask]()
    var renderedSidebarRowCount:Int {projectRows.count+chatRows.count+standaloneRows.count+activeRows.count+codeRows.count}
    private var rowProfile:PetGradientProfile?
    private var fileGlyphColors=[NSColor.systemBlue.cgColor,NSColor.systemPurple.cgColor,NSColor.white.cgColor]
    private var actionHits=[(CGRect,String)]()
    private weak var hoveredAttachment:AttachmentLayer?
    private var pendingTimeout:DispatchWorkItem?
    private(set) var pendingRequestID:String?
    private enum PendingSidebarTarget {case chat(String),project(String),newChat,newInProject}
    private var pendingSidebarTarget:PendingSidebarTarget?
    private let leftWidth:CGFloat=86, drawerWidth:CGFloat=78

    private let defaults:UserDefaults
    private var hiddenChats:Set<String>,hiddenProjects:Set<String>
    private var chatProjects:[String:String]
    init(defaults:UserDefaults = .standard) {
        self.defaults=defaults
        chatProjects=defaults.dictionary(forKey:"kaiChatProjects") as? [String:String] ?? [:]
        hiddenChats=Set(defaults.stringArray(forKey:"kaiHiddenChats") ?? [])
        hiddenProjects=Set(defaults.stringArray(forKey:"kaiHiddenProjects") ?? [])
        preview.onContentChanged={ [weak self] in self?.notifyPreviewContent() }
        layer.opacity=0; layer.addSublayer(left);layer.addSublayer(projectToggle);layer.addSublayer(preview); layer.addSublayer(main); layer.addSublayer(right);left.masksToBounds=true;right.masksToBounds=true;projectToggle.fillColor=nil;projectToggle.strokeColor=NSColor.white.withAlphaComponent(0.38).cgColor;projectToggle.lineWidth=1;projectToggle.lineCap = .round;projectToggle.lineJoin = .round
        configure(newRow,"+ new chat",8,.white.withAlphaComponent(0.65));left.addSublayer(newRow)
        configure(projectNewRow,"+ in project",7,.white.withAlphaComponent(0.58));left.addSublayer(projectNewRow)
        configure(standaloneTitle,"CHATS",7,.white.withAlphaComponent(0.42),.medium);left.addSublayer(standaloneTitle)
        configure(codeTitle,"CODE",7,.white.withAlphaComponent(0.42),.medium);left.addSublayer(codeTitle)
        configure(activeTitle,"ACTIVE",7,.white.withAlphaComponent(0.42),.medium);activeTitle.opacity=0;left.addSublayer(activeTitle)
        configure(message,"Choose a project and chat",9,.white.withAlphaComponent(0.68),.medium);main.addSublayer(message);configure(closeButton,"×",12,.white.withAlphaComponent(0.65));closeButton.alignmentMode = .center;main.addSublayer(closeButton);main.addSublayer(actionBar);main.addSublayer(settingsBar)
        input.cornerRadius=14; input.borderWidth=0.75; input.masksToBounds=true; layer.addSublayer(input);imageStrip.masksToBounds=true;layer.addSublayer(imageStrip)
        input.addSublayer(label("type or hold",8,.white.withAlphaComponent(0.46))); command.cornerRadius=6; command.borderWidth=0.7; command.addSublayer(label("⌘ command",7.4,.white.withAlphaComponent(0.74),.medium)); input.addSublayer(command); input.addSublayer(label("to speak",8,.white.withAlphaComponent(0.46)))
        send.type = .radial;send.startPoint=CGPoint(x:0.5,y:0.5);send.endPoint=CGPoint(x:1,y:0.5);send.locations=[0,0.52,1];send.cornerRadius=13; send.borderWidth=0.8; send.masksToBounds=true; input.addSublayer(send)
        sendShade.type = .radial;sendShade.colors=[NSColor.clear.cgColor,NSColor.black.withAlphaComponent(0.3).cgColor];sendShade.locations=[0,1];sendShade.startPoint=CGPoint(x:0.5,y:0.5);sendShade.endPoint=CGPoint(x:1,y:0.5);sendShade.cornerRadius=13;sendShade.masksToBounds=true;send.addSublayer(sendShade)
        sendGlass.type = .radial; sendGlass.cornerRadius=13; sendGlass.masksToBounds=true; sendGlass.colors=[NSColor.white.withAlphaComponent(0.18).cgColor,NSColor.white.withAlphaComponent(0.04).cgColor,NSColor.clear.cgColor]; sendGlass.locations=[0,0.42,1]; sendGlass.startPoint=CGPoint(x:0.3,y:0.76);sendGlass.endPoint=CGPoint(x:0.94,y:0.08);send.addSublayer(sendGlass)
        arrow.fillColor=nil;arrow.strokeColor=NSColor.white.withAlphaComponent(0.92).cgColor;arrow.lineWidth=1.15;arrow.lineCap = .round;arrow.lineJoin = .round;send.addSublayer(arrow)
        configure(voiceButton,"◉",14,.white.withAlphaComponent(0.7));voiceButton.alignmentMode = .center;input.addSublayer(voiceButton);voiceWaveform.contentsGravity = .resizeAspect;voiceWaveform.frame=CGRect(x:4,y:4,width:14,height:14);voiceButton.addSublayer(voiceWaveform)
        permissionGlyph.startPoint=CGPoint(x:0,y:0);permissionGlyph.endPoint=CGPoint(x:1,y:1);permissionGlyph.mask=permissionMask;permissionMask.contentsGravity = .resizeAspect;input.addSublayer(permissionGlyph)
        modelIcon.contentsGravity = .resizeAspect;modelChip.addSublayer(modelIcon);configure(modelName,"",7.8,.white.withAlphaComponent(0.84),.medium);modelName.truncationMode = .middle;modelChip.addSublayer(modelName);input.addSublayer(modelChip)
        effortChip.backgroundColor=NSColor.white.withAlphaComponent(0.11).cgColor;effortChip.cornerRadius=2;effortFill.anchorPoint=CGPoint(x:0,y:0.5);effortFill.startPoint=CGPoint(x:0,y:0.5);effortFill.endPoint=CGPoint(x:1,y:0.5);effortFill.cornerRadius=2;effortChip.addSublayer(effortShimmer);effortChip.addSublayer(effortFill);effortThumb.isHidden=true;effortThumb.cornerRadius=3;effortThumb.backgroundColor=NSColor.white.cgColor;effortChip.addSublayer(effortThumb);input.addSublayer(effortChip)
        configure(effortLabel,"",8,.white,.medium);effortFill.mask=effortLabel;fastBolt.contentsGravity = .resizeAspect;input.addSublayer(fastShimmer);fastTint.startPoint=CGPoint(x:0,y:0.5);fastTint.endPoint=CGPoint(x:1,y:0.5);fastTint.mask=fastBolt;input.addSublayer(fastTint);orchestratorChip.addSublayer(orchestratorShimmer);orchestratorGlyph.contentsGravity = .resizeAspect;orchestratorChip.addSublayer(orchestratorGlyph);input.addSublayer(orchestratorChip);configure(usageLabel,"",7,.white.withAlphaComponent(0.6));layer.addSublayer(usageLabel);usageTrack.cornerRadius=1;usageTrack.backgroundColor=NSColor.white.withAlphaComponent(0.12).cgColor;layer.addSublayer(usageTrack);usageFill.cornerRadius=1;usageFill.backgroundColor=NSColor.systemMint.cgColor;usageTrack.addSublayer(usageFill)
        pendingHighlight.opacity=0;pendingHighlight.cornerRadius=4;pendingHighlight.masksToBounds=true;pendingHighlight.backgroundColor=NSColor.white.withAlphaComponent(0.12).cgColor;pendingHighlight.borderColor=NSColor.white.withAlphaComponent(0.28).cgColor;pendingHighlight.borderWidth=0.6;pendingRibbon.colors=[NSColor.clear.cgColor,NSColor.white.withAlphaComponent(0.5).cgColor,NSColor.clear.cgColor];pendingRibbon.startPoint=CGPoint(x:0,y:0.5);pendingRibbon.endPoint=CGPoint(x:1,y:0.5);pendingHighlight.addSublayer(pendingRibbon);layer.addSublayer(pendingHighlight)
        installFlow()
    }

    private func beginPending(_ frame:CGRect,sidebarTarget:PendingSidebarTarget?=nil){
        pendingTimeout?.cancel();pendingRequestID=UUID().uuidString
        let id=pendingRequestID
        let timeout=DispatchWorkItem{[weak self] in guard let self,self.pendingRequestID == id else{return};self.cancelPending();if let id{self.onPendingTimeout?(id)};self.setCodexMessage("The chat could not be opened. Your draft is preserved.")}
        pendingTimeout=timeout;DispatchQueue.main.asyncAfter(deadline:.now()+12,execute:timeout)
        pendingHighlight.name="loadingHighlight"
        pendingSidebarTarget=sidebarTarget
        CATransaction.begin();CATransaction.setDisableActions(true);layer.addSublayer(pendingHighlight);pendingHighlight.isHidden=false;pendingHighlight.removeAllAnimations();pendingHighlight.frame=frame.insetBy(dx:-2,dy:-2);pendingHighlight.opacity=1;updatePendingSidebarHighlight();pendingRibbon.frame=CGRect(x:-34,y:0,width:34,height:pendingHighlight.bounds.height);CATransaction.commit()
        let sweep=CABasicAnimation(keyPath:"position.x");sweep.fromValue = -17;sweep.toValue=pendingHighlight.bounds.width+17;sweep.duration=0.95;sweep.repeatCount = .infinity;sweep.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);pendingRibbon.add(sweep,forKey:"loadingSweep")
        CATransaction.flush()
    }
    func beginChatLoading(_ name:String)->String {
        beginPending(CGRect(x:main.frame.minX+4,y:layer.bounds.height-21,width:max(24,main.bounds.width-8),height:15))
        setCodexMessage("Loading \(name.isEmpty ? "chat":name)…")
        return pendingRequestID!
    }
    func finishPending(requestID:String?){guard let requestID,requestID == pendingRequestID else{return};cancelPending()}
    func cancelPending(){pendingTimeout?.cancel();pendingTimeout=nil;pendingRequestID=nil;pendingSidebarTarget=nil;pendingRibbon.removeAnimation(forKey:"loadingSweep");let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=pendingHighlight.presentation()?.opacity ?? pendingHighlight.opacity;fade.toValue=0;fade.duration=0.18;CATransaction.begin();CATransaction.setDisableActions(true);pendingHighlight.opacity=0;CATransaction.commit();pendingHighlight.add(fade,forKey:"pendingFade")}

    private func updatePendingSidebarHighlight(){
        guard pendingRequestID != nil,let target=pendingSidebarTarget else{return}
        let row:CALayer?
        switch target {
        case .chat(let id):row=(chatRows+standaloneRows+activeRows+codeRows).first{$0.1 == id}?.0
        case .project(let path):row=projectRows.first{$0.1 == path}?.0
        case .newChat:row=newRow
        case .newInProject:row=projectNewRow.opacity>0 ? projectNewRow:nil
        }
        // The sidebar clips the highlight at its viewport. Track the logical
        // row ID so virtualization can remove and recreate its layer safely.
        CATransaction.begin();CATransaction.setDisableActions(true)
        if pendingHighlight.superlayer !== left{left.addSublayer(pendingHighlight)}
        pendingHighlight.isHidden = row == nil
        if let row{pendingHighlight.frame=row.frame.insetBy(dx:-2,dy:-2)}
        CATransaction.commit()
    }

    func layout(in frame:CGRect,bottomInset:CGFloat=0) {
        self.bottomInset=bottomInset
        layer.frame=frame; left.frame=CGRect(x:0,y:31,width:leftWidth,height:layer.bounds.height-31);projectToggle.frame=CGRect(x:0,y:2,width:12,height:16);layoutProjectToggle(animated:false)
        right.frame=CGRect(x:layer.bounds.width-drawerWidth,y:35,width:drawerWidth,height:layer.bounds.height-43);layoutProjects(animated:false); layoutMain(animated:false);layoutFiles()
        setFilesVisible(filesOpen,animated:false)
    }

    private func layoutProjects(animated:Bool) {
        defer{AdaptiveBackdrop.shared.refreshNativeInk()}
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        sidebarContentHeight=21+CGFloat(codeChats.isEmpty ? 0:21+codeChats.count*16)+CGFloat(standaloneChats.isEmpty ? 0:21+standaloneChats.count*16)+CGFloat(elsewhere.isEmpty ? 0:21+elsewhere.count*16)+CGFloat(projects.count)*21+(projects.contains(where:{$0.path == selectedProject}) ? CGFloat(chats.count)*16+19:0)
        projectOffset=min(projectOffset,max(0,sidebarContentHeight-left.bounds.height+8))
        var cursor=left.bounds.height-4+projectOffset
        let viewport=left.bounds.insetBy(dx:0,dy:-18)
        let oldProjects=Dictionary(projectRows.map{($0.1,$0.0)},uniquingKeysWith:{first,_ in first})
        let oldChats=Dictionary((chatRows+standaloneRows+activeRows+codeRows).map{($0.1,$0.0)},uniquingKeysWith:{first,_ in first})
        var nextProjects=[(CALayer,String)](),nextChats=[(ChatRowLayer,String)](),nextStandalone=[(ChatRowLayer,String)](),nextActive=[(ChatRowLayer,String)](),nextCode=[(ChatRowLayer,String)]()
        func chat(_ id:String,_ name:String,_ running:Bool,_ rect:CGRect)->(ChatRowLayer,String)? {
            guard viewport.intersects(rect) else{return nil}
            let row:ChatRowLayer
            if let existing=oldChats[id],existing.name == name {row=existing}
            else {row=ChatRowLayer(name);row.name=name;left.addSublayer(row)}
            setFrame(row,rect,animated:animated);row.setSelected(id == selectedThread)
            let agent=(chats+standaloneChats+codeChats).first{$0.id==id}?.agent ?? elsewhere.first{$0.id==id}?.agent ?? ProviderIcons.identifier(name) ?? "codex"
            row.setProvider(agent,profile:rowProfile);row.setRunning(running,runID:modelState?.activeTasks.first{$0.id == id}?.runId,successfulRuns:(modelState?.finishedTasks ?? []).filter{$0.id == id}.compactMap(\.runId))
            row.setPet(id.hasPrefix("child:") ? taskPets[id]:nil)
            return (row,id)
        }
        setFrame(newRow,CGRect(x:5,y:cursor-12,width:leftWidth-10,height:11),animated:animated);cursor-=21
        projectNewRow.opacity=projects.contains(where:{$0.path == selectedProject}) ? 1:0
        activeTitle.opacity=elsewhere.isEmpty ? 0:1
        if !elsewhere.isEmpty {
            setFrame(activeTitle,CGRect(x:5,y:cursor-10,width:leftWidth-10,height:9),animated:animated);cursor-=15
            for item in elsewhere {if let row=chat(item.id,item.name,true,CGRect(x:4,y:cursor-13,width:leftWidth-8,height:12)){nextActive.append(row)};cursor-=16}
            cursor-=6
        }
        codeTitle.opacity=codeChats.isEmpty ? 0:1
        if !codeChats.isEmpty {
            setFrame(codeTitle,CGRect(x:5,y:cursor-10,width:leftWidth-10,height:9),animated:animated);cursor-=15
            for item in codeChats {if let row=chat(item.id,item.name,item.status == "active",CGRect(x:9,y:cursor-13,width:leftWidth-13,height:12)){nextCode.append(row)};cursor-=16}
            cursor-=6
        }
        standaloneTitle.opacity=standaloneChats.isEmpty ? 0:1
        if !standaloneChats.isEmpty {
            setFrame(standaloneTitle,CGRect(x:5,y:cursor-10,width:leftWidth-10,height:9),animated:animated);cursor-=15
            for item in standaloneChats {if let row=chat(item.id,item.name,item.status == "active",CGRect(x:9,y:cursor-13,width:leftWidth-13,height:12)){nextStandalone.append(row)};cursor-=16}
            cursor-=6
        }
        for project in projects {
            let rect=CGRect(x:4,y:cursor-17,width:leftWidth-8,height:16)
            if viewport.intersects(rect) {
                let row=oldProjects[project.path] ?? CALayer()
                if row.superlayer == nil {let folder=FileGlyphLayer(size:CGSize(width:16,height:13)),title=label(project.name,8,.white.withAlphaComponent(0.72)),hide=label("×",10,.white.withAlphaComponent(0.65));row.addSublayer(folder);row.addSublayer(title);row.addSublayer(hide);hide.name="hide";hide.opacity=0;hide.frame=CGRect(x:leftWidth-20,y:2,width:12,height:12);folder.frame=CGRect(x:0,y:1,width:16,height:13);title.frame=CGRect(x:19,y:2,width:leftWidth-42,height:11);left.addSublayer(row)}
                row.sublayers?.compactMap{$0 as? CATextLayer}.first?.string=project.name
                row.name=project.path;setFrame(row,rect,animated:animated);nextProjects.append((row,project.path))
            }
            cursor-=21
            if project.path == selectedProject {
                for item in chats {if let row=chat(item.id,item.name,item.status == "active",CGRect(x:18,y:cursor-13,width:leftWidth-21,height:12)){nextChats.append(row)};cursor-=16}
                setFrame(projectNewRow,CGRect(x:19,y:cursor-12,width:leftWidth-22,height:11),animated:animated);cursor-=19
            }
        }
        var kept=Set((nextProjects.map{$0.0}+(nextChats+nextStandalone+nextActive).map{$0.0 as CALayer}).map{ObjectIdentifier($0)})
        kept.formUnion(nextCode.map{ObjectIdentifier($0.0)})
        for row in projectRows.map({$0.0})+(chatRows+standaloneRows+activeRows+codeRows).map({$0.0 as CALayer}) where !kept.contains(ObjectIdentifier(row)){row.removeFromSuperlayer()}
        projectRows=nextProjects;chatRows=nextChats;standaloneRows=nextStandalone;activeRows=nextActive;codeRows=nextCode
        updatePendingSidebarHighlight()
        projectIntrudesComposer=cursor+left.frame.minY<36
    }

    private func layoutMain(animated:Bool) {
        defer{AdaptiveBackdrop.shared.refreshNativeInk()}
        defer{notifyPreviewContent()}
        let paired=previewOpen && previewFromFiles && filesOpen
        let previewWidth:CGFloat=paired ? max(94,min(126,layer.bounds.width*0.35)):leftWidth
        let fileX:CGFloat=paired ? layer.bounds.width-previewWidth-drawerWidth-9:layer.bounds.width-drawerWidth
        setFrame(right,CGRect(x:fileX,y:35,width:drawerWidth,height:layer.bounds.height-43),animated:animated)
        let x=previewOpen || projectsCollapsed ? 11:leftWidth+11
        let rightInset=paired ? layer.bounds.width-fileX+4:(previewOpen ? leftWidth+8:(filesOpen ? drawerWidth+10:0))
        let target=CGRect(x:x,y:0,width:max(0,layer.bounds.width-x-rightInset),height:layer.bounds.height)
        setFrame(preview,CGRect(x:layer.bounds.width-previewWidth-4,y:31,width:previewWidth,height:layer.bounds.height-39),animated:animated)
        setFrame(main,target,animated:animated); let w=target.width,h=target.height
        message.frame=CGRect(x:5,y:h-18,width:max(0,w-26),height:12);closeButton.frame=CGRect(x:max(0,w-20),y:h-21,width:16,height:18);layoutPathHits();actionBar.frame=CGRect(x:5,y:h-37,width:max(0,w-10),height:13);layoutActions();settingsBar.frame=CGRect(x:5,y:h-53,width:max(0,w-10),height:13);layoutSettings()
        let minX=(usesUnifiedComposer || projectIntrudesComposer) && !projectsCollapsed && !previewOpen ? left.frame.maxX+9:18,maxX=paired ? fileX-4:(previewOpen ? layer.bounds.width-leftWidth-8:(filesOpen ? layer.bounds.width-drawerWidth-9:layer.bounds.width-18)),composerW=min(260,max(70,maxX-minX)),idealX=(minX+maxX-composerW)/2,inputX=min(max(idealX,minX),maxX-composerW),composerFrame=CGRect(x:inputX,y:usesUnifiedComposer ? 14:0,width:composerW,height:usesUnifiedComposer ? 58:28);setFrame(input,composerFrame,animated:animated);composerFrameChanged?(composerFrame,animated);setFrame(imageStrip,CGRect(x:inputX+8,y:usesUnifiedComposer ? 78:34,width:max(0,composerW-16),height:32),animated:animated);layoutImages();let texts=input.sublayers?.compactMap{$0 as? CATextLayer} ?? []
        if usesUnifiedComposer {
            texts.first?.string=composerW<190 ? "type or hold":"Type, or hold";texts.first?.frame=CGRect(x:10,y:36,width:60,height:12)
            command.frame=CGRect(x:composerW<190 ? 70:74,y:32,width:61,height:19);command.sublayers?.first?.frame=CGRect(x:5,y:4,width:54,height:11)
            if texts.count>1{texts[1].string=composerW>=230 ? "to speak":"";texts[1].frame=CGRect(x:139,y:36,width:43,height:11)}
        } else {
            texts.first?.string="type or hold";texts.first?.frame=CGRect(x:11,y:8,width:55,height:11);command.frame=CGRect(x:68,y:5,width:61,height:18);command.sublayers?.first?.frame=CGRect(x:4,y:4,width:54,height:10)
            if texts.count>1{texts[1].string=composerW>=230 ? "to speak":"";texts[1].frame=CGRect(x:134,y:8,width:43,height:11)}
        }
        setFrame(send,CGRect(x:composerW-28,y:composerControlsCenterY-12.5,width:25,height:25),animated:animated);voiceButton.frame=CGRect(x:composerW-53,y:composerControlsCenterY-11,width:22,height:22);sendShade.frame=send.bounds;sendGlass.frame=send.bounds;let p=CGMutablePath();p.move(to:CGPoint(x:9,y:14.5));p.addLine(to:CGPoint(x:13,y:18.5));p.addLine(to:CGPoint(x:17,y:14.5));p.move(to:CGPoint(x:13,y:18.5));p.addLine(to:CGPoint(x:13,y:7.5));arrow.path=p;arrow.frame=send.bounds
        layoutComposerControls()
        let conversationBottom:CGFloat=(usesUnifiedComposer ? 14:0)+((imageStrip.sublayers?.isEmpty == false) ? (usesUnifiedComposer ? 100:68):(usesUnifiedComposer ? 62:32))
        conversationFrameChanged?(CGRect(x:target.minX+2,y:conversationBottom,width:max(0,target.width-4),height:max(0,h-56-conversationBottom)))
    }

    func handleClick(_ point:CGPoint,count:Int=1)->Bool {
        let popoverGeneration=composerPopover.generation
        defer{composerPopover.dismiss(ifUnchanged:popoverGeneration)}
        if count>=2,let url=fileURL(at:point){onFileActions?(url);return true}
        // Keep overlapping image previews' existing front-to-back ownership.
        let attachmentPoint=CGPoint(x:point.x-imageStrip.frame.minX,y:point.y-imageStrip.frame.minY)
        if imageStrip.frame.contains(point),let attachment=((hoveredAttachment?.frame.contains(attachmentPoint) == true ? hoveredAttachment:nil) ?? imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer}.reversed().first{$0.frame.contains(attachmentPoint)}) {
            let local=CGPoint(x:attachmentPoint.x-attachment.frame.minX,y:attachmentPoint.y-attachment.frame.minY)
            if attachment.closeContains(local){remove(attachment)}
            return true
        }
        if usesUnifiedComposer,contextHit.contains(point){showUsagePopover();return true}
        if usesUnifiedComposer,usageLabel.frame.union(usageTrack.frame).insetBy(dx:-4,dy:-3).contains(point){showUsagePopover();return true}
        var targets=[(frame:CGRect,limit:CGRect,action:()->Void)]()
        func add(_ rect:CGRect,limit:CGRect?=nil,_ action:@escaping()->Void){
            targets.append((rect,limit ?? layer.bounds,action))
        }
        func inMain(_ rect:CGRect)->CGRect{rect.offsetBy(dx:main.frame.minX,dy:main.frame.minY)}
        for (rect,id) in settingsHits {
            let frame=inMain(rect.offsetBy(dx:settingsBar.frame.minX,dy:settingsBar.frame.minY))
            add(frame,limit:main.frame){[self] in
                if id == "context"{lightweightContext.toggle();onContextMode?(lightweightContext);layoutSettings()}
                else{showSettingsMenu(id)}
            }
        }
        add(inMain(closeButton.frame),limit:main.frame){[self] in onClose?()}
        if !previewOpen{add(projectToggle.frame){[self] in toggleProjects()}}
        for (rect,id) in composerHits {add(rect.offsetBy(dx:input.frame.minX,dy:input.frame.minY),limit:input.frame){[self] in if id=="permissions"{showPermissionsMenu()}else if id=="fast"{onFast?()}else if id=="orchestrator"{onOrchestrator?()}else{showSettingsMenu(id)}}}
        let composerActions:[(CGRect,()->Void)]=[(send.frame,{[self] in onSend?()}),(voiceButton.frame,{[self] in onVoice?()})]
        for (frame,action) in composerActions {
            add(frame.offsetBy(dx:input.frame.minX,dy:input.frame.minY),limit:input.frame.insetBy(dx:0,dy:-4),action)
        }
        let typingArea=CGRect(x:input.frame.minX,y:input.frame.minY,width:max(0,voiceButton.frame.minX-6),height:input.frame.height)
        add(typingArea,limit:input.frame){[self] in onFocusComposer?()}
        for (rect,id) in actionHits {
            let frame=inMain(rect.offsetBy(dx:actionBar.frame.minX,dy:actionBar.frame.minY))
            add(frame,limit:main.frame){[self] in
                if id != "review"{beginPending(frame)}
                switch id {case "back":onNavigate?("back");case "forward":onNavigate?("forward");case "new":onNew?();case "approve":onApprove?();case "reject":onReject?();case "approval_stop":onApprovalStop?();case "review":onReview?();case "interrupt":onInterrupt?();default:cancelPending()}
            }
        }
        if previewOpen {
            if let url=preview.representedURL {
                for (rect,action) in preview.actionTargets {add(rect.offsetBy(dx:preview.frame.minX,dy:preview.frame.minY),limit:preview.frame){[self] in onFileAction?(action,url)}}
            }
            add(preview.closeTarget.offsetBy(dx:preview.frame.minX,dy:preview.frame.minY),limit:preview.frame){[self] in setPreviewVisible(false)}
        }
        if filesOpen {
            for row in fileRows {
                let frame=row.frame.offsetBy(dx:right.frame.minX,dy:right.frame.minY)
                for (rect,action) in row.actionTargets {add(rect.offsetBy(dx:frame.minX,dy:frame.minY),limit:right.frame){[self] in onFileAction?(action,row.url)}}
                let body=CGRect(x:frame.minX,y:frame.minY,width:max(0,frame.width-26),height:frame.height)
                add(body,limit:right.frame){[self] in
                    beginPending(frame)
                    if row.isDirectory{onFolder?(row.url.path)}else{openPreview(row.url,pendingID:pendingRequestID)}
                }
            }
        }
        for (rect,url) in pathHits {add(inMain(rect),limit:main.frame){[self] in openPreview(url)}}
        if !projectsCollapsed && !previewOpen {
            add(newRow.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY),limit:left.frame){[self] in
                beginPending(newRow.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY),sidebarTarget:.newChat);onNew?()
            }
            for (row,id) in (standaloneRows+activeRows+chatRows+codeRows).map({($0.0 as CALayer,$0.1)})+projectRows {
                let frame=row.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY)
                let project=projectRows.contains{$0.0 === row}
                // Separate the row's normal action from its hide affordance.
                let body=CGRect(x:frame.minX,y:frame.minY,width:max(0,frame.width-12),height:frame.height)
                let hide=CGRect(x:frame.maxX-12,y:frame.minY,width:12,height:frame.height)
                add(body,limit:left.frame){[self] in
                    beginPending(frame,sidebarTarget:project ? .project(id):.chat(id))
                    if project{onProject?(id)}else{onChat?(id)}
                }
                add(hide,limit:left.frame){[self] in if project{hideProject(id)}else{hideChat(id)}}
            }
            if projects.contains(where:{$0.path == selectedProject}) {
                add(projectNewRow.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY),limit:left.frame){[self] in
                    beginPending(projectNewRow.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY),sidebarTarget:.newInProject);onNewInProject?()
                }
            }
        }
        if let index=NotchInputRegion.nearestTarget(at:point,frames:targets.map(\.frame),limits:targets.map(\.limit)) {
            targets[index].action();return true
        }
        return layer.bounds.contains(point)
    }

    func fileAction(at point:CGPoint)->(action:String,url:URL)? {
        var frames=[CGRect](),limits=[CGRect](),actions=[(String,URL)?]()
        func add(_ rect:CGRect,limit:CGRect,action:(String,URL)?=nil){frames.append(rect);limits.append(limit);actions.append(action)}
        if previewOpen,let url=preview.representedURL {
            for (rect,action) in preview.actionTargets{add(rect.offsetBy(dx:preview.frame.minX,dy:preview.frame.minY),limit:preview.frame,action:(action,url))}
            add(preview.closeTarget.offsetBy(dx:preview.frame.minX,dy:preview.frame.minY),limit:preview.frame)
        }
        if filesOpen {
            for row in fileRows {
                let frame=row.frame.offsetBy(dx:right.frame.minX,dy:right.frame.minY)
                for (rect,action) in row.actionTargets{add(rect.offsetBy(dx:frame.minX,dy:frame.minY),limit:right.frame,action:(action,row.url))}
                add(CGRect(x:frame.minX,y:frame.minY,width:max(0,frame.width-26),height:frame.height),limit:right.frame)
            }
        }
        guard let index=NotchInputRegion.nearestTarget(at:point,frames:frames,limits:limits),let result=actions[index] else{return nil}
        return (result.0,result.1)
    }
    func handleHover(_ point:CGPoint?) {
        let actionPoint=point.map{CGPoint(x:$0.x-main.frame.minX-actionBar.frame.minX,y:$0.y-main.frame.minY-actionBar.frame.minY)}
        let action=actionPoint.flatMap{p in actionHits.first{$0.0.insetBy(dx:0,dy:-3).contains(p)}?.1}
        if action != hoveredAction,let action,let engine=actionSymbols[action]{engine.showScene(action=="approve" ? "success":action=="reject" ? "failure":action=="review" ? "review":"pause",looping:false)};hoveredAction=action
        let overEffort=point.map{usesUnifiedComposer && !effortChip.isHidden && effortChip.frame.offsetBy(dx:input.frame.minX,dy:input.frame.minY).contains($0)} ?? false
        if overEffort && !hoveringEffort{effortShimmer.play(restart:true)};hoveringEffort=overEffort
        let overFast=point.map{usesUnifiedComposer && !fastTint.isHidden && fastTint.frame.insetBy(dx:-4,dy:-4).offsetBy(dx:input.frame.minX,dy:input.frame.minY).contains($0)} ?? false
        if overFast && !hoveringFast{fastShimmer.play(restart:true)};hoveringFast=overFast
        let fileHover=point.flatMap{fileAction(at:$0)}
        preview.setActionHover(preview.representedURL == fileHover?.url ? fileHover?.action:nil)
        for row in fileRows{row.setActionHover(row.url == fileHover?.url ? fileHover?.action:nil)}
        guard let point else {projectRows.forEach{$0.0.sublayers?.last?.opacity=0}; (chatRows+standaloneRows+activeRows+codeRows).forEach{$0.0.setHovered(false)};hoveredAttachment=nil;for(i,item) in (imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer} ?? []).enumerated(){item.setHovered(false);item.zPosition=CGFloat(i)};return }
        let attachmentPoint=CGPoint(x:point.x-imageStrip.frame.minX,y:point.y-imageStrip.frame.minY),previews=imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer} ?? [],hovered=(hoveredAttachment?.frame.contains(attachmentPoint) == true ? hoveredAttachment:previews.reversed().first{$0.frame.contains(attachmentPoint)})
        hoveredAttachment=hovered;for(i,item) in previews.enumerated(){item.setHovered(item === hovered);item.zPosition=item === hovered ? 1000:CGFloat(i)}
        let p=CGPoint(x:point.x-left.frame.minX,y:point.y-left.frame.minY)
        let rows=projectRows.map{$0.0}+(chatRows+standaloneRows+codeRows+activeRows).map{$0.0 as CALayer}
        let hoveredRow=(!projectsCollapsed && !previewOpen) ? NotchInputRegion.nearestTarget(at:p,frames:rows.map(\.frame),limits:Array(repeating:left.bounds,count:rows.count)).map{rows[$0]}:nil
        for (row,_) in projectRows {row.sublayers?.last?.opacity=row === hoveredRow ? 1:0}
        for (row,_) in chatRows+standaloneRows+codeRows+activeRows {row.setHovered(row === hoveredRow)}
    }

    func fileURL(at point:CGPoint)->URL? {
        if previewOpen,preview.frame.contains(point){return preview.representedURL}
        if filesOpen,right.frame.contains(point) {
            let local=CGPoint(x:point.x-right.frame.minX,y:point.y-right.frame.minY)
            if let index=NotchInputRegion.nearestTarget(at:local,frames:fileRows.map(\.frame),limits:Array(repeating:right.bounds,count:fileRows.count)){return fileRows[index].url}
        }
        return nil
    }
    private func notifyPreviewContent(){
        let rect=CGRect(x:preview.frame.minX+3,y:preview.frame.minY+2,width:max(0,preview.frame.width-6),height:max(0,preview.frame.height-19))
        previewContentChanged?(rect,preview.textContent,preview.representedURL,previewOpen && layer.opacity>0)
    }
    func scroll(_ delta:CGFloat,at point:CGPoint) { if filesOpen,right.frame.contains(point){let overflow=max(0,CGFloat(fileEntries.count)*18-right.bounds.height+8);fileOffset=min(overflow,max(0,fileOffset-delta));layoutFiles()}else if left.frame.contains(point){let total=sidebarContentHeight;projectOffset=min(max(0,total-left.bounds.height+8),max(0,projectOffset-delta));layoutProjects(animated:false)} }

    func toggleFiles() { if previewOpen && !previewFromFiles{setPreviewVisible(false)};filesOpen.toggle();if !filesOpen{previewFromFiles=false};setFilesVisible(filesOpen,animated:true);onFilesVisibleChanged?(filesOpen);layoutMain(animated:true) }
    var isFilesOpen:Bool { filesOpen }
    func resetFiles() { if previewOpen{setPreviewVisible(false)};filesOpen=false;fileOffset=0;setFilesVisible(false,animated:false);onFilesVisibleChanged?(false);layoutFiles();layoutMain(animated:false) }

    func setCodexMessage(_ text:String){guard (message.string as? NSAttributedString)?.string != text else{return};let font=AppTypography.ui(ofSize:8),result=NSMutableAttributedString(string:text,attributes:[.font:font,.foregroundColor:NSColor.white.withAlphaComponent(0.62)]),pattern=#"(?:file://)?(?:/Users/|Users/|~/)[^\s\]\[\)\(>,]+"#;pathHits.removeAll();if let regex=try? NSRegularExpression(pattern:pattern){for match in regex.matches(in:text,range:NSRange(text.startIndex...,in:text)){result.addAttributes([.font:AppTypography.ui(ofSize:8,weight:.bold),.underlineStyle:NSUnderlineStyle.single.rawValue,.foregroundColor:NSColor.white.withAlphaComponent(0.9)],range:match.range)}};message.string=result;layoutPathHits()}
    func hideChat(_ id:String){
        hiddenChats.insert(id);defaults.set(Array(hiddenChats).sorted(),forKey:"kaiHiddenChats")
        if let state=modelState {setState(state)}
    }
    func hideProject(_ path:String){
        hiddenProjects.insert(path);defaults.set(Array(hiddenProjects).sorted(),forKey:"kaiHiddenProjects")
        if let state=modelState {setState(state)}
    }
    func setState(_ state:KaiState){
        let oldIntrusion=projectIntrudesComposer
        var membershipChanged=false
        for chat in state.chats where chatProjects[chat.id] != state.project {chatProjects[chat.id]=state.project;membershipChanged=true}
        if membershipChanged {defaults.set(chatProjects,forKey:"kaiChatProjects")}
        let nextProjects=state.projects.filter{!hiddenProjects.contains($0.path)}
        let active=state.activeTasks.filter{!hiddenChats.contains($0.id) && !$0.id.hasPrefix("dot:") && !(chatProjects[$0.id].map{hiddenProjects.contains($0)} ?? false)}
        let activeIDs=Set(active.map(\.id))
        let nextChats=hiddenProjects.contains(state.project) ? [] : state.chats.filter{!hiddenChats.contains($0.id) && !activeIDs.contains($0.id)}
        let nextStandalone=state.standaloneChats.filter{!hiddenChats.contains($0.id) && !activeIDs.contains($0.id)}
        let nextCode=(state.codeChats ?? []).filter{!hiddenChats.contains($0.id) && !activeIDs.contains($0.id)}
        let sidebarChanged=codeChats != nextCode || selectedProject != state.project || selectedThread != state.threadId || projects != nextProjects || chats != nextChats || standaloneChats != nextStandalone || elsewhere != active || modelState?.finishedTasks != state.finishedTasks
        if modelState?.ownerToken != state.ownerToken || modelState?.selectedModel != state.selectedModel{composerPopover.dismiss()}
        modelState=state;agentPicker.update(state)
        selectedProject=state.project;selectedThread=state.threadId;approvalNeeded=state.approval;turnActive=state.activeTurnId != nil
        projects=nextProjects;chats=nextChats;standaloneChats=nextStandalone;codeChats=nextCode
        if elsewhere.isEmpty && !active.isEmpty {projectOffset=0}
        elsewhere=active
        if let files=state.files {
            if fileEntries.count != files.count || zip(fileEntries,files).contains(where:{$0.path != $1.path || $0.collapsed != $1.collapsed}) {
                fileEntries=files;fileOffset=min(fileOffset,max(0,CGFloat(fileEntries.count)*18-right.bounds.height+8));layoutFiles()
            }
        }
        let title=state.threadId == nil ? "Choose a chat" : (state.threadName.isEmpty ? "Chat" : state.threadName)
        let focus=state.activeTasks.first(where:{$0.id == state.threadId})
        let percent=(state.threadId?.hasPrefix("code:") == true ? nil:state.rateLimitRemainingPercent).map{" · \($0)%"} ?? ""
        let name=title
        let selectedApproval=state.approval && (state.approvalThreadId == nil || state.approvalThreadId == state.threadId)
        let activityText=selectedApproval ? "Approval needed" : (state.viewedActivity.scene == "off" ? "" : state.viewedActivity.text)
        let shownReadOnly=state.readOnly && (focus == nil || focus?.id == state.threadId)
        let loadingText=state.projectsLoading == true ? (state.notice.hasPrefix("Could not refresh projects") ? " · projects unavailable, retrying":" · loading projects") : ""
        let codeNotice=state.threadId?.hasPrefix("code:") == true && (state.notice.hasPrefix("Code:") || state.notice.hasPrefix("Loading Code") || state.notice.hasPrefix("Code disconnected") || state.notice.hasPrefix("Prompt staged")) ? " · "+state.notice:""
        setCodexMessage(name+percent+loadingText+codeNotice+(activityText.isEmpty ? "":" · "+activityText)+(shownReadOnly ? " · read only":""))
        message.truncationMode = .middle
        if sidebarChanged {layoutProjects(animated:false)}
        // Streaming text does not change geometry. Relayout would reset the
        // scroll view and recreate controls on every packet.
        if oldIntrusion != projectIntrudesComposer {layoutMain(animated:false)}
        else {layoutActions();layoutSettings()}
    }
    func setContextMode(_ lightweight:Bool){lightweightContext=lightweight;layoutSettings()}
    private func layoutSettings() {
        layoutComposerControls()
        guard let state=modelState else{return}
        let model=state.models?.first{$0.model == state.selectedModel || $0.model == state.effectiveModel || ($0.resolvedModel != nil && $0.resolvedModel == state.effectiveModel)}
        let name=state.effectiveModel ?? model?.resolvedModel ?? model?.displayName ?? state.selectedModel ?? "…"
        let effort=state.effectiveReasoningEffort ?? state.reasoningEffort ?? ""
        let key=[state.selectedAgent ?? "",String(Double(settingsBar.bounds.width)),name,effort,String(lightweightContext),String(state.threadId?.hasPrefix("code:") == true),String(Double(renderingScale))]
        guard key != settingsLayoutKey else{return};settingsLayoutKey=key
        settingsBar.sublayers?.forEach{$0.removeFromSuperlayer()};settingsHits.removeAll()
        var x:CGFloat=0
        let codeView=state.threadId?.hasPrefix("code:") == true
        let contextTitle=lightweightContext ? "Context: light":"Context: screen"
        let settings=usesUnifiedComposer ? [("agent",state.agents?.first{$0.id == state.selectedAgent}?.name ?? "Agent"),("context",contextTitle)]:codeView ? [("model",name),("context",contextTitle)]:[("model",name),("effort",effort),("context",contextTitle)]
        for (id,text) in settings {
            let width=min(id == "model" ? 200: id == "effort" ? 48:80, min((text as NSString).size(withAttributes:[.font:AppTypography.ui(ofSize:7.5,weight:.medium)]).width+9,max(0,settingsBar.bounds.width-x)))
            guard width>16 else{break}
            let mask=label(text,7.5,.white,.medium);mask.frame=CGRect(x:0,y:1,width:width,height:11)
            let gradient=CAGradientLayer();gradient.frame=CGRect(x:x,y:0,width:width,height:13);gradient.startPoint=CGPoint(x:0,y:0);gradient.endPoint=CGPoint(x:1,y:1);gradient.mask=mask
            let lower=name.lowercased()
            let colors:[NSColor]=lower.contains("astra") ? [.systemPurple,.systemBlue,.systemPink]:lower.contains("luna") ? [.gray,.white,.lightGray]:lower.contains("terra") ? [.systemBlue,.systemGreen,.systemTeal]:lower.contains("sol") ? [.systemYellow,.systemOrange,.systemRed]:[.systemTeal,.systemBlue,.systemPurple]
            let levels=["none","minimal","low","medium","high","xhigh","max","ultra"]
            let intensity=CGFloat(levels.firstIndex(of:effort) ?? 3)/7
            gradient.colors=(id == "effort" ? [NSColor.white.withAlphaComponent(0.45+intensity*0.55),NSColor.systemPurple.withAlphaComponent(0.4+intensity*0.6),NSColor.systemPink.withAlphaComponent(0.4+intensity*0.6)]:colors).map(\.cgColor)
            settingsBar.addSublayer(gradient);if EmbeddedCodeRuntime.shared.active || !codeView || id == "context"{settingsHits.append((gradient.frame,id))};x+=width+6
        }
    }
    private func selectedEffortModel(_ state:KaiState)->KaiModel? {
        state.models?.first{$0.model==state.selectedModel} ?? state.models?.first{$0.model==state.effectiveModel || ($0.resolvedModel != nil && $0.resolvedModel==state.effectiveModel)}
    }
    private func effortMenuItem(_ model:KaiModel,state:KaiState)->NSMenuItem {let item=NSMenuItem(title:"Thinking intensity…",action:#selector(openThinkingPanel),keyEquivalent:"");item.target=self;return item}
    func performKeyboardCommand(_ id:String)->Bool{switch id{case "model":showSettingsMenu("model");case "thinking":showThinkingPopover();case "usage":showUsagePopover();default:return false};return true}
    @objc private func openThinkingPanel(){showThinkingPopover()}
    private func applyComposerFrameStyle(){
        CATransaction.begin();CATransaction.setDisableActions(true)
        input.colors=[NSColor.white.withAlphaComponent(0.085).cgColor,NSColor.white.withAlphaComponent(0.035).cgColor]
        input.borderColor=NSColor.white.withAlphaComponent(0.16).cgColor
        command.borderWidth=borderlessComposer ? 0:0.7
        command.backgroundColor=borderlessComposer ? nil:rowProfile.flatMap{profile in profile.gradients.ambient.stops.first.map{alpha(color($0.color),0.14)}}
        CATransaction.commit()
    }
    private func layoutComposerControls(){
        let shown=usesUnifiedComposer
        permissionGlyph.isHidden = !shown;modelChip.isHidden = !shown;effortChip.isHidden = !shown
        guard shown,let state=modelState else{composerHits=[];return}
        setVoiceActive(voiceActive)
        let name=state.effectiveModel ?? state.selectedModel ?? state.providerDefaultModel ?? "Model"
        let levels=selectedEffortModel(state)?.supportedReasoningEfforts.map(\.reasoningEffort) ?? []
        let effort=state.reasoningEffort.flatMap{$0.isEmpty ? nil:$0} ?? state.effectiveReasoningEffort ?? ""
        var key:[String]=[name,effort,levels.joined(separator:","),state.workspaceAccess ?? "",state.workspaceApprovalPolicy ?? ""]
        key += [String(state.borderlessComposer == true),String(Double(input.bounds.width)),String(state.fastMode == true),String(state.fastSupported == true),String(state.orchestratorMode == true),String(state.orchestratorAvailable == true)]
        key += [state.usageSummary ?? "",String(state.contextWindow?.fraction ?? -1),String(Double(renderingScale)),String(Double(bottomInset)),String(Double(input.frame.minY))]
        guard key != composerControlKey else{return};composerControlKey=key
        applyComposerFrameStyle()
        let width=input.bounds.width,hasEffort = !levels.isEmpty && width>=215
        permissionGlyph.frame=CGRect(x:9,y:composerControlsCenterY-7.5,width:15,height:15);permissionMask.frame=permissionGlyph.bounds
        let access=state.workspaceAccess ?? "read-only",symbol=access=="full" ? "lock.open.fill":access=="read-only" ? "lock.fill":"shield.lefthalf.filled"
        permissionMask.contents=NSImage(systemSymbolName:symbol,accessibilityDescription:nil)?.withSymbolConfiguration(.init(paletteColors:[.white]))
        let colors:[NSColor]=access=="full" ? [.white,.systemRed]:access=="workspace" || access=="native" ? [.white,.systemOrange]:[.white,.lightGray]
        let oldColors=permissionGlyph.presentation()?.colors ?? permissionGlyph.colors
        permissionGlyph.colors=colors.map(\.cgColor)
        let transition=CABasicAnimation(keyPath:"colors");transition.fromValue=oldColors;transition.toValue=permissionGlyph.colors;transition.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0:0.25;permissionGlyph.add(transition,forKey:"sandboxLevel")
        let fast=state.fastSupported == true,orchestrator=state.orchestratorAvailable == true;let title=ModelIdentity.title(name),identityWidth:CGFloat=ModelIdentity.image(name)==nil ? 0:18
        let titleWidth=(title as NSString).size(withAttributes:[.font:AppTypography.ui(ofSize:7.8,weight:.medium)]).width
        let modelWidth=min(identityWidth+ceil(titleWidth)+2,max(16,width-91-(hasEffort ? 52:0)-(fast ? 22:0)-(orchestrator ? 22:0)));modelChip.frame=CGRect(x:30,y:composerControlsCenterY-8.5,width:modelWidth,height:17)
        let image=ModelIdentity.image(name);modelIcon.contentsScale=renderingScale;permissionMask.contentsScale=renderingScale;modelIcon.contents=image;modelIcon.frame=CGRect(x:0,y:2,width:13,height:13);modelIcon.isHidden=image==nil
        let textX:CGFloat=image==nil ? 0:18;modelName.frame=CGRect(x:textX,y:1.5,width:max(0,modelWidth-textX),height:14);modelName.setNeedsDisplay();modelName.string=ModelIdentity.title(name);modelName.contentsScale=renderingScale
        effortChip.isHidden = !hasEffort;effortChip.backgroundColor=nil;effortChip.frame=CGRect(x:modelChip.frame.maxX+3,y:composerControlsCenterY-9,width:50,height:18)
        let fraction=CGFloat(levels.firstIndex(of:effort) ?? 0)/CGFloat(max(1,levels.count-1)),color=EffortAppearance.color(effort,fraction:fraction)
        effortShimmer.frame=effortChip.bounds;effortShimmer.configure(size:effortChip.bounds.size,tint:color);let shimmerKey=name+"|"+effort
        if shimmerKey != effortShimmerKey{effortShimmerKey=shimmerKey;if layer.opacity>0 && hasEffort{effortShimmer.play(restart:true)}else{effortShimmer.stop()}}
        effortFill.frame=effortChip.bounds;effortFill.colors=[color.cgColor,EffortAppearance.color(min(1,fraction+0.25)).cgColor];effortLabel.frame=CGRect(x:0,y:2,width:50,height:14);effortLabel.setNeedsDisplay();effortLabel.string=effort=="xhigh" ? "Extra high":effort.capitalized;effortLabel.contentsScale=renderingScale
        fastTint.isHidden = !fast;fastTint.frame=CGRect(x:(hasEffort ? effortChip.frame.maxX:modelChip.frame.maxX)+5,y:composerControlsCenterY-8.5,width:17,height:17);fastBolt.frame=fastTint.bounds;fastBolt.contents=NSImage(systemSymbolName:state.fastMode == true ? "bolt.fill":"bolt",accessibilityDescription:"Fast mode")?.withSymbolConfiguration(.init(paletteColors:[.white]));fastTint.colors=(state.fastMode == true ? [NSColor(calibratedRed:1,green:0.94,blue:0.60,alpha:1),NSColor(calibratedRed:1,green:0.74,blue:0.21,alpha:1)]:[NSColor.secondaryLabelColor,NSColor.secondaryLabelColor]).map(\.cgColor)
        fastShimmer.isHidden = !fast;fastShimmer.frame=fastTint.frame.insetBy(dx:-4,dy:-1);fastShimmer.configure(size:fastShimmer.bounds.size,tint:NSColor(calibratedRed:1,green:0.85,blue:0.41,alpha:1));fastBolt.shadowOpacity=0
        if fastWasOn != state.fastMode{fastWasOn=state.fastMode;if layer.opacity>0 && fast{fastShimmer.play(restart:true)}else{fastShimmer.stop()}}
        orchestratorChip.isHidden = !orchestrator;orchestratorChip.frame=CGRect(x:(fast ? fastTint.frame.maxX:(hasEffort ? effortChip.frame.maxX:modelChip.frame.maxX))+5,y:composerControlsCenterY-9,width:20,height:18);orchestratorChip.cornerRadius=4;orchestratorChip.backgroundColor=state.orchestratorMode == true && !borderlessComposer ? NSColor.systemPurple.withAlphaComponent(0.12).cgColor:nil;orchestratorGlyph.frame=CGRect(x:3,y:2,width:14,height:14);orchestratorGlyph.contents=NSImage(systemSymbolName:"point.3.connected.trianglepath.dotted",accessibilityDescription:"Orchestrator")?.withSymbolConfiguration(.init(paletteColors:[state.orchestratorMode == true ? .systemPurple:.secondaryLabelColor]));orchestratorShimmer.frame=orchestratorChip.bounds;orchestratorShimmer.configure(size:orchestratorShimmer.bounds.size,tint:.systemPurple)
        if orchestratorWasOn != state.orchestratorMode{orchestratorWasOn=state.orchestratorMode;if state.orchestratorMode == true && layer.opacity>0{orchestratorShimmer.play(restart:true)}else{orchestratorShimmer.stop()}}
        let footerCenter=(input.frame.minY-bottomInset)/2
        usageLabel.string=state.usageSummary ?? "";usageLabel.frame=CGRect(x:input.frame.minX+42,y:footerCenter-7,width:max(0,width-42),height:14);usageLabel.setNeedsDisplay();usageTrack.frame=CGRect(x:input.frame.minX+5,y:footerCenter-1,width:30,height:2);usageFill.frame=CGRect(x:0,y:0,width:30*CGFloat(state.usageWindows?.first?.fraction ?? 0),height:2);usageTrack.isHidden=state.usageWindows?.isEmpty != false
        if contextTrack.superlayer==nil{layer.addSublayer(contextTrack);layer.addSublayer(contextRing);layer.addSublayer(contextText)}
        let context=state.contextWindow
        contextTrack.isHidden=context==nil;contextRing.isHidden=context==nil;contextText.isHidden=context==nil
        if let context{
            let x=input.frame.maxX-65,y=footerCenter-5
            contextHit=CGRect(x:x-3,y:y-3,width:68,height:18)
            let circle=CGPath(ellipseIn:CGRect(x:x,y:y,width:10,height:10),transform:nil)
            for shape in [contextTrack,contextRing]{shape.frame=layer.bounds;shape.path=circle;shape.fillColor=nil;shape.lineWidth=1.5}
            contextTrack.strokeColor=NSColor.white.withAlphaComponent(0.16).cgColor
            contextRing.strokeColor=EffortAppearance.color(CGFloat(context.fraction)).cgColor
            let old=contextRing.presentation()?.strokeEnd ?? contextRing.strokeEnd;contextRing.strokeEnd=context.fraction
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{let animation=CABasicAnimation(keyPath:"strokeEnd");animation.fromValue=old;animation.toValue=context.fraction;animation.duration=0.25;contextRing.add(animation,forKey:"context")}
            contextText.contentsScale=renderingScale;contextText.font=AppTypography.ui(ofSize:7);contextText.fontSize=7;contextText.foregroundColor=NSColor.white.withAlphaComponent(0.7).cgColor;contextText.string=String(format:"%.0f%%",context.fraction*100);contextText.frame=CGRect(x:x+15,y:y-1,width:45,height:12)
            usageLabel.frame.size.width=max(0,width-114)
        }else{contextHit = .zero}
        composerHits=[(permissionGlyph.frame.insetBy(dx:-4,dy:-4),"permissions"),(modelChip.frame,"model")]
        if orchestrator{composerHits.append((orchestratorChip.frame,"orchestrator"))}
        if fast{composerHits.append((fastTint.frame.insetBy(dx:-3,dy:-4),"fast"))}
        if hasEffort{composerHits.append((effortChip.frame.insetBy(dx:-4,dy:-9),"effort"))}
    }
    func thinkingRows()->[EffortTileRow]{
        guard let state=modelState,let model=selectedEffortModel(state)else{return []};let levels=model.supportedReasoningEfforts.map(\.reasoningEffort),current=state.reasoningEffort.flatMap{$0.isEmpty ? nil:$0} ?? state.effectiveReasoningEffort
        return levels.enumerated().map{i,value in let row=EffortTileRow(value:value,title:value=="xhigh" ? "Extra high":value.capitalized,index:i,count:levels.count,selected:value==current);row.commit={[weak self] value in guard let self,self.modelState?.ownerToken==state.ownerToken,self.modelState?.selectedModel==state.selectedModel else{return};self.onSettings?(state.selectedModel ?? "",value);self.composerPopover.dismiss()};return row}
    }
    private func showThinkingPopover(){let rows=thinkingRows();if !rows.isEmpty{composerPopover.show(key:"effort",title:"Thinking intensity",rows:rows,at:NSEvent.mouseLocation)}}
    private func showUsagePopover(){
        guard let state=modelState else{return};var rows:[NSView]=[]
        for text in [state.usageAccount,state.usageUpdated].compactMap({$0})+(state.usageWindows ?? []).map({$0.label+" · "+$0.remaining+($0.reset.map{"\n"+$0} ?? "")}) {let row=NSTextField(wrappingLabelWithString:text);row.font=AppTypography.ui(ofSize:10,weight:.medium);rows.append(row)}
        if let resets=state.usageResets{rows.append(NSTextField(labelWithString:"Banked resets · \(resets.availableCount) available"));for credit in resets.credits{let expiry=credit.expiresAt.map{Date(timeIntervalSince1970:$0>10_000_000_000 ? $0/1000:$0).formatted(date:.abbreviated,time:.shortened)};let text=(credit.title ?? "Full reset")+(expiry.map{" · expires "+$0} ?? "");let row=NSButton(title:text,target:self,action:#selector(useUsageReset(_:)));row.identifier=NSUserInterfaceItemIdentifier(credit.id);row.bezelStyle = .inline;row.isEnabled=credit.status=="available";rows.append(row)}}
        let refresh=NSButton(title:"Refresh usage",target:self,action:#selector(refreshUsage));refresh.bezelStyle = .inline;rows.append(refresh)
        composerPopover.show(key:"usage",title:(state.agents?.first{$0.id==state.selectedAgent}?.name ?? "Agent")+" usage",rows:rows,at:NSEvent.mouseLocation)
    }
    @objc private func useUsageReset(_ button:NSButton){guard let id=button.identifier?.rawValue,modelState?.usageResets?.credits.contains(where:{$0.id==id&&$0.status=="available"})==true else{return};composerPopover.dismiss();onUsageReset?(id)}
    @objc private func refreshUsage(){composerPopover.dismiss();onUsageRefresh?()}
    private func showPermissionsMenu(){composerPopover.dismiss();permissionMenu()?.popUp(positioning:nil,at:NSEvent.mouseLocation,in:nil)}
    func permissionMenu()->NSMenu? {
        guard let state=modelState else{return nil};let menu=NSMenu()
        let path=menu.addItem(withTitle:state.workspaceExecutionPath ?? state.project,action:nil,keyEquivalent:"");path.isEnabled=false
        for (value,title) in [("read-only","Read only"),("workspace","Workspace edits")]+(state.selectedAgent=="codex" ? [("full","Full access")]:[]) {
            let item=menu.addItem(withTitle:title,action:#selector(selectPermission(_:)),keyEquivalent:"");item.target=self;item.representedObject=["access",value,state.ownerToken ?? ""];item.state=(state.workspaceAccess==value || value=="workspace" && state.workspaceAccess=="native") ? .on:.off
        }
        if state.selectedAgent=="codex" {menu.addItem(.separator());for (value,title) in [("ask","Ask me"),("auto-review","Review for me"),("never","Never ask")] {let item=menu.addItem(withTitle:title,action:#selector(selectPermission(_:)),keyEquivalent:"");item.target=self;item.representedObject=["approvals",value,state.ownerToken ?? ""];item.state=state.workspaceApprovalPolicy==value ? .on:.off}}
        menu.addItem(.separator());let hint=menu.addItem(withTitle:"Changes apply to the next turn or voice call",action:nil,keyEquivalent:"");hint.isEnabled=false
        return menu
    }
    @objc private func selectPermission(_ item:NSMenuItem){guard let values=item.representedObject as? [String],values.count==3,values[2]==(modelState?.ownerToken ?? "") else{return};onPermissions?(values[0]=="access" ? values[1]:nil,values[0]=="approvals" ? values[1]:nil)}
    func dismissComposerPickers(){agentPicker.dismiss();composerPopover.dismiss()}
    func showAgentPicker(_ kind:String){
        guard let state=modelState else{return};composerPopover.dismiss()
        agentPicker.onAgent={[weak self] id in self?.onAgent?(id)}
        agentPicker.onModel={[weak self] id in self?.onModel?(id)}
        agentPicker.onEffort={[weak self] model,effort in self?.onSettings?(model,effort)}
        agentPicker.show(scope:kind=="agent" ? .providers:kind=="models" ? .models:.all,state:state,at:NSEvent.mouseLocation)
    }
    private func showSettingsMenu(_ kind:String){if kind=="model" || kind=="agent"{showAgentPicker(kind);return};if kind=="effort"{showThinkingPopover();return};composerPopover.dismiss();settingsMenu(kind)?.popUp(positioning:nil,at:NSEvent.mouseLocation,in:nil)}
    func settingsMenu(_ kind:String)->NSMenu? {
        guard let state=modelState,let models=state.models else{return nil}
        let menu=NSMenu()
        if kind == "agent" {
            for agent in state.agents ?? [] {
                let item=menu.addItem(withTitle:agent.name,action:#selector(selectAgent(_:)),keyEquivalent:"")
                item.target=self;item.representedObject=agent.id;item.isEnabled=agent.available;item.state=agent.id == state.selectedAgent ? .on:.off
            }
            return menu
        }
        if kind == "model" {
            let providerName=state.agents?.first{$0.id == state.selectedAgent}?.name ?? "Agent"
            if EmbeddedCodeRuntime.shared.active{let item=menu.addItem(withTitle:(state.threadId != nil ? "Continue · "+(state.effectiveModel ?? state.selectedModel ?? providerName):"Use "+(state.providerDefaultModel ?? state.effectiveModel ?? providerName)),action:#selector(selectSetting(_:)),keyEquivalent:"");item.target=self;item.representedObject=["","",state.ownerToken ?? ""];item.state=state.selectedModel == nil ? .on:.off}
            for model in models {
                let item=menu.addItem(withTitle:ModelIdentity.title(model.displayName),action:#selector(selectSetting(_:)),keyEquivalent:"");item.target=self;item.image=ModelIdentity.image(model.resolvedModel ?? model.model);item.image?.size=CGSize(width:16,height:16);item.representedObject=[model.model,"",state.ownerToken ?? ""];item.state=model.model == state.selectedModel ? .on:.off
            }
            if let model=selectedEffortModel(state),!model.supportedReasoningEfforts.isEmpty{menu.addItem(.separator());menu.addItem(effortMenuItem(model,state:state))}
        } else if let model=selectedEffortModel(state),!model.supportedReasoningEfforts.isEmpty {
            menu.addItem(effortMenuItem(model,state:state))
        }
        return menu
    }
    @objc private func selectAgent(_ item:NSMenuItem){if let id=item.representedObject as? String{onAgent?(id)}}
    @objc private func selectSetting(_ item:NSMenuItem) {guard let values=item.representedObject as? [String],values.count == 3,values[2]==(modelState?.ownerToken ?? "") else{return};if values[1].isEmpty,let onModel{onModel(values[0])}else{onSettings?(values[0],values[1])}}

    private func layoutActions(){
        let key=[String(Double(actionBar.bounds.width)),String(modelState?.canGoBack == true),String(modelState?.canGoForward == true),String(approvalNeeded),modelState?.approvalRequestId ?? "",String(turnActive),String(modelState?.threadId?.hasPrefix("code:") == true),String(Double(renderingScale))]
        guard key != actionsLayoutKey else{return};actionsLayoutKey=key
        actionBar.sublayers?.forEach{$0.removeFromSuperlayer()};actionHits.removeAll();var x:CGFloat=0
        var actions=[(String,String)]()

        if modelState?.canGoBack == true{actions.append(("back","‹"))}
        if modelState?.canGoForward == true{actions.append(("forward","›"))}
        if approvalNeeded{actions += [("approve","Approve"),("reject","Reject"),("approval_stop","Stop chat"),("review","Review request")]}
        if turnActive && !approvalNeeded{actions.append(("interrupt","Stop"))}
        let externalCode = !EmbeddedCodeRuntime.shared.active && modelState?.threadId?.hasPrefix("code:") == true
        actions.append(("new",externalCode ? "+ Codex chat":"+ New"))
        for (id,title) in actions {
            let scene=["approve":"success","reject":"failure","approval_stop":"pause","review":"review"][id],width:CGFloat=scene == nil ? CGFloat(title.count)*5+10:24
            if x+width>actionBar.bounds.width{break}
            if let scene{let engine=actionSymbols[id] ?? NotchIndicatorEngine();actionSymbols[id]=engine;engine.layer.name="notch-action-"+id;engine.layer.frame=CGRect(x:x,y:-2,width:24,height:16);actionBar.addSublayer(engine.layer);if let rowProfile{engine.setProfile(rowProfile,wave:false)};engine.layout();engine.showScene(scene,looping:false)}
            else{let item=label(title,7.5,.white.withAlphaComponent(0.74),.medium);item.frame=CGRect(x:x+4,y:1,width:width-8,height:11);actionBar.addSublayer(item)}
            actionHits.append((CGRect(x:x,y:-2,width:width,height:17),id));x+=width+3
        }
    }
    func previewFile(_ url:URL){openPreview(url)}
    func refreshFileAction(_ action:String,url:URL){
        CATransaction.begin();CATransaction.setDisableActions(true)
        for row in fileRows where row.url == url{row.refreshAction(action)}
        if preview.representedURL == url{preview.refreshAction(action)}
        CATransaction.commit()
    }

    func setComposerHasText(_ hasText:Bool){CATransaction.begin();CATransaction.setDisableActions(true);for item in input.sublayers ?? [] where item === command || (item is CATextLayer && item !== voiceButton){item.opacity=hasText ? 0:1};CATransaction.commit()}
    func synchronizeImages(_ images:[NSImage]){imageStrip.sublayers?.forEach{$0.removeFromSuperlayer()};for image in images{imageStrip.addSublayer(AttachmentLayer(image))};layoutImages()}
    func clearImages(){imageStrip.sublayers?.forEach{$0.removeFromSuperlayer()};layoutImages();imagesChanged()}
    func addImages(_ images:[NSImage]){for image in images{let preview=AttachmentLayer(image);preview.opacity=0;imageStrip.addSublayer(preview);let reveal=CAAnimationGroup();let fade=CABasicAnimation(keyPath:"opacity"),scale=CABasicAnimation(keyPath:"transform.scale");fade.fromValue=0;fade.toValue=1;scale.fromValue=0.82;scale.toValue=1;reveal.animations=[fade,scale];reveal.duration=0.34;reveal.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);preview.opacity=1;preview.add(reveal,forKey:"attach")};layoutImages();imagesChanged()}
    private func layoutImages(){let previews=imageStrip.sublayers ?? [],w:CGFloat=42,step=previews.count<2 ? 0:min(w+5,max(13,(imageStrip.bounds.width-w)/CGFloat(previews.count-1)));CATransaction.begin();CATransaction.setDisableActions(true);for(i,preview) in previews.enumerated(){preview.frame=CGRect(x:CGFloat(i)*step,y:0,width:w,height:32);preview.zPosition=CGFloat(i)};imageStrip.opacity=previews.isEmpty ? 0:1;CATransaction.commit()}
    private func remove(_ preview:AttachmentLayer){let fade=CABasicAnimation(keyPath:"opacity"),scale=CABasicAnimation(keyPath:"transform.scale");fade.fromValue=preview.presentation()?.opacity ?? 1;fade.toValue=0;scale.fromValue=1;scale.toValue=0.78;let group=CAAnimationGroup();group.animations=[fade,scale];group.duration=0.18;group.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);preview.add(group,forKey:"remove");DispatchQueue.main.asyncAfter(deadline:.now()+0.18){[weak self,weak preview] in preview?.removeFromSuperlayer();self?.layoutImages();self?.imagesChanged()}}
    private func imagesChanged(){onImagesChanged?(imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer}.map(\.image) ?? [])}

    private func setFilesVisible(_ visible:Bool,animated:Bool) {
        let duration=animated ? 0.42:0, opacity:Float=visible ? 1:0, offset:CGFloat=visible ? 0:10
        for item in [right] { let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=item.presentation()?.opacity ?? item.opacity;fade.toValue=opacity;fade.duration=duration;fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);item.opacity=opacity;CATransaction.commit();if animated{item.add(fade,forKey:"filesFade")} }
        let slide=CABasicAnimation(keyPath:"transform.translation.x");slide.fromValue=right.presentation()?.transform.m41 ?? (visible ? 10:0);slide.toValue=offset;slide.duration=duration;slide.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);right.setAffineTransform(CGAffineTransform(translationX:offset,y:0));CATransaction.commit();if animated{right.add(slide,forKey:"filesSlide")}
    }

    private func layoutFiles(){fileRows.forEach{$0.removeFromSuperlayer()};fileRows.removeAll();let first=max(0,Int(fileOffset/18)-1),last=min(fileEntries.count,first+Int(right.bounds.height/18)+4);guard first<last else{return};CATransaction.begin();CATransaction.setDisableActions(true);for i in first..<last {let entry=fileEntries[i],indent=min(CGFloat(min(entry.depth,8))*5,max(0,drawerWidth-66)),row=FileRowLayer(URL(fileURLWithPath:entry.path),isDirectory:entry.directory,open:entry.directory && !entry.collapsed);row.string=entry.name;row.frame=CGRect(x:3+indent,y:right.bounds.height-15+fileOffset-CGFloat(i)*18,width:max(24,drawerWidth-6-indent),height:14);row.setColors(fileGlyphColors,animated:false);right.addSublayer(row);fileRows.append(row)};CATransaction.commit()}

    private func openPreview(_ url:URL,pendingID:String?=nil){previewFromFiles=filesOpen;preview.show(url){[weak self] in if let pendingID{self?.finishPending(requestID:pendingID)}};if previewOpen{layoutMain(animated:true)}else{setPreviewVisible(true)}}
    private func setPreviewVisible(_ visible:Bool){guard visible != previewOpen else{return};previewOpen=visible;if !visible{previewFromFiles=false};let duration=0.46,curve=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);func transition(_ item:CALayer,opacity:Float,from fallback:CGFloat,to offset:CGFloat){let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=item.presentation()?.opacity ?? item.opacity;fade.toValue=opacity;fade.duration=duration;fade.timingFunction=curve;let slide=CABasicAnimation(keyPath:"transform.translation.x");slide.fromValue=item.presentation()?.transform.m41 ?? fallback;slide.toValue=offset;slide.duration=duration;slide.timingFunction=curve;CATransaction.begin();CATransaction.setDisableActions(true);item.opacity=opacity;item.setAffineTransform(CGAffineTransform(translationX:offset,y:0));CATransaction.commit();item.add(fade,forKey:"previewFade");item.add(slide,forKey:"previewSlide")};transition(left,opacity:visible || projectsCollapsed ? 0:1,from:visible ? 0:-16,to:visible || projectsCollapsed ? -16:0);transition(preview,opacity:visible ? 1:0,from:visible ? leftWidth*0.62:0,to:visible ? 0:leftWidth*0.62);projectToggle.opacity=visible ? 0:1;layoutProjectToggle(animated:true);layoutMain(animated:true)}
    private func toggleProjects(){projectsCollapsed.toggle();let target:Float=projectsCollapsed ? 0:1,offset:CGFloat=projectsCollapsed ? -16:0,fade=CABasicAnimation(keyPath:"opacity"),slide=CABasicAnimation(keyPath:"transform.translation.x"),curve=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);fade.fromValue=left.presentation()?.opacity ?? left.opacity;fade.toValue=target;fade.duration=0.42;fade.timingFunction=curve;slide.fromValue=left.presentation()?.transform.m41 ?? left.transform.m41;slide.toValue=offset;slide.duration=0.42;slide.timingFunction=curve;CATransaction.begin();CATransaction.setDisableActions(true);left.opacity=target;left.setAffineTransform(CGAffineTransform(translationX:offset,y:0));CATransaction.commit();left.add(fade,forKey:"projectFade");left.add(slide,forKey:"projectSlide");layoutProjectToggle(animated:true);layoutMain(animated:true)}
    private func layoutProjectToggle(animated:Bool){let right=projectsCollapsed,p=CGMutablePath();p.move(to:CGPoint(x:right ? 4:8,y:4));p.addLine(to:CGPoint(x:right ? 8:4,y:8));p.addLine(to:CGPoint(x:right ? 4:8,y:12));let old=projectToggle.presentation()?.path ?? projectToggle.path;CATransaction.begin();CATransaction.setDisableActions(true);projectToggle.path=p;CATransaction.commit();if animated{let morph=CABasicAnimation(keyPath:"path");morph.fromValue=old;morph.toValue=p;morph.duration=0.3;morph.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);projectToggle.add(morph,forKey:"direction")}}
    private func layoutPathHits(){pathHits.removeAll();guard let attributed=message.string as? NSAttributedString else{return};let text=attributed.string,font=AppTypography.ui(ofSize:8),pattern=#"(?:file://)?(?:/Users/|Users/|~/)[^\s\]\[\)\(>,]+"#;guard let regex=try? NSRegularExpression(pattern:pattern) else{return};for match in regex.matches(in:text,range:NSRange(text.startIndex...,in:text)){let before=(text as NSString).substring(to:match.range.location),path=(text as NSString).substring(with:match.range),x=(before as NSString).size(withAttributes:[.font:font]).width,w=(path as NSString).size(withAttributes:[.font:AppTypography.ui(ofSize:8,weight:.bold)]).width;let url:URL?;if path.hasPrefix("file://"){url=URL(string:path)}else{url=URL(fileURLWithPath:path.hasPrefix("Users/") ? "/"+path:(path as NSString).expandingTildeInPath)};if let url{pathHits.append((CGRect(x:message.frame.minX+x,y:message.frame.minY,width:w,height:13),url))}}}

    private func collapse(_ group:CALayer,_ expanded:Bool) { let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=group.presentation()?.opacity ?? group.opacity;fade.toValue=expanded ? 1:0;fade.duration=0.26;fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);group.opacity=expanded ? 1:0;group.setAffineTransform(.identity);CATransaction.commit();group.add(fade,forKey:"collapse") }

    func setVisible(_ visible:Bool,duration:Double) { if !visible{orchestratorShimmer.stop();fastShimmer.stop();effortShimmer.stop();composerPopover.dismiss();handleHover(nil)}else {if !effortChip.isHidden{effortShimmer.play(restart:true)};if !fastTint.isHidden{fastShimmer.play(restart:true)};if orchestratorWasOn == true{orchestratorShimmer.play(restart:true)}};let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=layer.presentation()?.opacity ?? layer.opacity;fade.toValue=visible ? 1:0;fade.duration=duration*0.72;fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);layer.opacity=visible ? 1:0;CATransaction.commit();layer.add(fade,forKey:"visibility");notifyPreviewContent() }

    func setProfile(_ profile:PetGradientProfile,animated:Bool) {
        defer{if usesUnifiedComposer{applyComposerFrameStyle()}}
        rowProfile=profile
        actionSymbols.values.forEach{$0.setProfile(profile,wave:false)}
        let s=profile.gradients.ambient.stops;guard !s.isEmpty else{return};let c=[s.first!,s[s.count/2],s.last!].map{color($0.color)},radial=c.sorted{brightness($0)<brightness($1)};fileGlyphColors=c.map{readable($0)};fileRows.forEach{$0.setColors(fileGlyphColors,animated:animated)};animate(input,to:[alpha(c[0],0.15),alpha(c[1],0.065),alpha(c[2],0.1)],animated ? 0.72:0);animate(response,to:c.map{alpha(readable($0),0.92)},animated ? 0.72:0);animate(send,to:[mix(radial[2],NSColor.white.cgColor,0.2),mix(radial[1],NSColor.black.cgColor,0.16),mix(radial[0],NSColor.black.cgColor,0.58)],animated ? 0.72:0)
        CATransaction.begin();CATransaction.setDisableActions(true);input.borderColor=alpha(c[2],0.22);command.backgroundColor=alpha(c[0],0.14);command.borderColor=alpha(c[2],0.22);send.borderColor=NSColor.white.withAlphaComponent(0.36).cgColor;CATransaction.commit()
    }

    private func setFrame(_ item:CALayer,_ target:CGRect,animated:Bool) { let old=item.presentation()?.frame ?? item.frame;CATransaction.begin();CATransaction.setDisableActions(true);item.frame=target;CATransaction.commit();guard animated else{return};let pos=CABasicAnimation(keyPath:"position");pos.fromValue=NSValue(point:NSPoint(x:old.midX,y:old.midY));pos.toValue=NSValue(point:NSPoint(x:target.midX,y:target.midY));pos.duration=0.42;pos.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);item.add(pos,forKey:"position");if old.size != target.size { let bounds=CABasicAnimation(keyPath:"bounds");bounds.fromValue=NSValue(rect:NSRect(origin:.zero,size:old.size));bounds.toValue=NSValue(rect:NSRect(origin:.zero,size:target.size));bounds.duration=0.42;bounds.timingFunction=pos.timingFunction;item.add(bounds,forKey:"bounds") } }
    private func label(_ value:String,_ size:CGFloat,_ color:NSColor,_ weight:NSFont.Weight = .regular)->CATextLayer { let t=CATextLayer();configure(t,value,size,color,weight);return t }
    private func configure(_ t:CATextLayer,_ value:String,_ size:CGFloat,_ color:NSColor,_ weight:NSFont.Weight = .regular) { t.string=value;t.font=AppTypography.ui(ofSize:size,weight:weight);t.fontSize=size;t.foregroundColor=color.cgColor;t.contentsScale=renderingScale;t.truncationMode = .end }
    private func installFlow(){response.startPoint=CGPoint(x:0,y:0.5);response.endPoint=CGPoint(x:1,y:0.5);response.locations=[0,0.45,1];let a=CABasicAnimation(keyPath:"locations");a.fromValue=[-0.12,0.28,0.88];a.toValue=[0.12,0.72,1.12];a.duration=2.8;a.autoreverses=true;a.repeatCount = .infinity;a.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);response.add(a,forKey:"flow")}
    private func installTyping(){let now=CACurrentMediaTime();for(i,dot) in typingDots.enumerated(){dot.frame=CGRect(x:CGFloat(i)*5.8+1,y:3.4,width:2.6,height:2.6);dot.cornerRadius=1.3;dot.backgroundColor=NSColor.white.cgColor;let jump=CAKeyframeAnimation(keyPath:"transform.translation.y");jump.values=[0,0,2.4,0,0];jump.keyTimes=[0,0.16,0.34,0.52,1];jump.duration=1.18;jump.beginTime=now+Double(i)*0.14;jump.repeatCount = .infinity;jump.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);dot.add(jump,forKey:"typing")}}
    private func animate(_ g:CAGradientLayer,to target:[CGColor],_ duration:Double){let from=(g.presentation()?.colors as? [CGColor]) ?? (g.colors as? [CGColor]) ?? target;CATransaction.begin();CATransaction.setDisableActions(true);g.colors=target;CATransaction.commit();guard duration>0 else{return};let a=CAKeyframeAnimation(keyPath:"colors");a.values=(0..<18).map{i in zip(from,target).map{mix($0.0,$0.1,CGFloat(i)/17)}};a.duration=duration;a.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);g.add(a,forKey:"theme")}
    private func mix(_ a:CGColor,_ b:CGColor,_ t:CGFloat)->CGColor{let x=NSColor(cgColor:a)?.usingColorSpace(.sRGB) ?? .clear,y=NSColor(cgColor:b)?.usingColorSpace(.sRGB) ?? .clear;var ah:CGFloat=0,asat:CGFloat=0,ab:CGFloat=0,aa:CGFloat=0,bh:CGFloat=0,bs:CGFloat=0,bb:CGFloat=0,ba:CGFloat=0;x.getHue(&ah,saturation:&asat,brightness:&ab,alpha:&aa);y.getHue(&bh,saturation:&bs,brightness:&bb,alpha:&ba);var d=bh-ah;if d>0.5{d-=1};if d < -0.5{d+=1};let h=(ah+d*t).truncatingRemainder(dividingBy:1);return NSColor(calibratedHue:h<0 ? h+1:h,saturation:asat+(bs-asat)*t,brightness:ab+(bb-ab)*t,alpha:aa+(ba-aa)*t).cgColor}
    private func color(_ hex:String)->CGColor{let v=UInt64(hex.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((v>>16)&255)/255,green:CGFloat((v>>8)&255)/255,blue:CGFloat(v&255)/255,alpha:1).cgColor}
    private func brightness(_ c:CGColor)->CGFloat{let x=NSColor(cgColor:c)?.usingColorSpace(.sRGB) ?? .black;var h:CGFloat=0,s:CGFloat=0,b:CGFloat=0,a:CGFloat=0;x.getHue(&h,saturation:&s,brightness:&b,alpha:&a);return b}
    private func readable(_ c:CGColor)->CGColor{let x=NSColor(cgColor:c)?.usingColorSpace(.sRGB) ?? .white;var h:CGFloat=0,s:CGFloat=0,b:CGFloat=0,a:CGFloat=0;x.getHue(&h,saturation:&s,brightness:&b,alpha:&a);return NSColor(calibratedHue:h,saturation:min(s,0.82),brightness:max(b,0.74),alpha:1).cgColor}
    private func alpha(_ c:CGColor,_ a:CGFloat)->CGColor{(NSColor(cgColor:c) ?? .clear).withAlphaComponent(a).cgColor}
}
