import AppKit
import CoreText
import ImageIO
import QuickLookThumbnailing

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
    private let icon:FileTypeGlyphLayer,title=CATextLayer()
    init(_ url:URL,isDirectory:Bool,open:Bool){self.url=url;self.isDirectory=isDirectory;icon=FileTypeGlyphLayer(url,isDirectory:isDirectory,open:open);super.init();title.string=url.lastPathComponent;title.font=NSFont.systemFont(ofSize:8);title.fontSize=8;title.foregroundColor=NSColor.white.withAlphaComponent(0.52).cgColor;title.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;title.truncationMode = .end;addSublayer(icon);addSublayer(title)}
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){let source=layer as? FileRowLayer;url=source?.url ?? URL(fileURLWithPath:"/");isDirectory=source?.isDirectory ?? false;icon=FileTypeGlyphLayer(url,isDirectory:isDirectory,open:false);super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();icon.frame=CGRect(x:0,y:0.5,width:15.5,height:13);title.frame=CGRect(x:18,y:2,width:max(0,bounds.width-18),height:11)}
    var string:String {get{title.string as? String ?? ""}set{title.string=newValue}}
    func setColors(_ colors:[CGColor],animated:Bool){icon.setColors(colors,animated:animated)}
}

private final class FilePreviewLayer:CALayer {
    private let title=CATextLayer(),body=CATextLayer(),image=CALayer(),close=CALayer(),mark=CAShapeLayer()
    private var representedURL:URL?,loadID=UUID()
    override init(){super.init();opacity=0;masksToBounds=true;title.font=NSFont.systemFont(ofSize:7.5,weight:.semibold);title.fontSize=7.5;title.foregroundColor=NSColor.white.withAlphaComponent(0.8).cgColor;title.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;title.truncationMode = .middle;addSublayer(title);body.font=NSFont.monospacedSystemFont(ofSize:5.7,weight:.regular);body.fontSize=5.7;body.foregroundColor=NSColor.white.withAlphaComponent(0.48).cgColor;body.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;body.isWrapped=true;body.truncationMode = .end;addSublayer(body);image.contentsGravity = .resizeAspect;image.cornerRadius=5;image.masksToBounds=true;addSublayer(image);close.backgroundColor=NSColor.white.withAlphaComponent(0.09).cgColor;close.cornerRadius=5;addSublayer(close);mark.fillColor=nil;mark.strokeColor=NSColor.white.withAlphaComponent(0.72).cgColor;mark.lineWidth=0.9;mark.lineCap = .round;close.addSublayer(mark)}
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();title.frame=CGRect(x:3,y:bounds.height-14,width:max(0,bounds.width-19),height:10);close.frame=CGRect(x:bounds.width-13,y:bounds.height-14,width:10,height:10);mark.frame=close.bounds;let p=CGMutablePath();p.move(to:CGPoint(x:3,y:3));p.addLine(to:CGPoint(x:7,y:7));p.move(to:CGPoint(x:7,y:3));p.addLine(to:CGPoint(x:3,y:7));mark.path=p;body.frame=CGRect(x:3,y:2,width:bounds.width-6,height:max(0,bounds.height-19));image.frame=CGRect(x:3,y:3,width:bounds.width-6,height:max(0,bounds.height-21))}
    func closeContains(_ point:CGPoint)->Bool{close.frame.insetBy(dx:-3,dy:-3).contains(point)}
    func show(_ url:URL,onReady:@escaping ()->Void){
        representedURL=url;loadID=UUID();let token=loadID
        title.string=url.lastPathComponent;image.contents=nil;image.opacity=0;body.opacity=1;body.string="loading…"
        let textExtensions=Set(["swift","md","txt","json","plist","py","js","ts","tsx","css","html","yaml","yml","sh","toml","csv"])
        DispatchQueue.global(qos:.userInitiated).async { [weak self] in
            if textExtensions.contains(url.pathExtension.lowercased()) {
                let result=Result {String(decoding:try Data(contentsOf:url,options:.mappedIfSafe).prefix(3200),as:UTF8.self)}
                DispatchQueue.main.async {guard let self,self.loadID==token else{return};switch result{case .success(let text):self.body.string=text;case .failure(let error):self.failure(error)};onReady()}
                return
            }
            if let source=CGImageSourceCreateWithURL(url as CFURL,nil),let picture=CGImageSourceCreateImageAtIndex(source,0,nil){
                DispatchQueue.main.async {guard let self,self.loadID==token else{return};self.body.opacity=0;self.image.contents=picture;self.image.opacity=1;onReady()}
                return
            }
            let request=QLThumbnailGenerator.Request(fileAt:url,size:CGSize(width:180,height:180),scale:NSScreen.main?.backingScaleFactor ?? 2,representationTypes:.all)
            QLThumbnailGenerator.shared.generateBestRepresentation(for:request){result,error in DispatchQueue.main.async {guard let self,self.loadID==token else{return};if let result{self.body.opacity=0;self.image.contents=result.nsImage;self.image.opacity=1}else{self.failure(error)};onReady()}}
        }
    }
    private func failure(_ error:Error?){body.opacity=1;image.opacity=0;body.string=error == nil ? "preview unavailable":"unable to preview\n\(error!.localizedDescription)"}
}

private final class ChatRowLayer:CALayer {
    private let title=CATextLayer(),dots=(0..<3).map{_ in CALayer()},value:String,font=NSFont.systemFont(ofSize:8)
    private var hovered=false
    init(_ value:String) {
        self.value=value;super.init();masksToBounds=true;title.string=value;title.font=font;title.fontSize=8;title.foregroundColor=NSColor.white.withAlphaComponent(0.42).cgColor;title.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;title.truncationMode = .end;addSublayer(title)
        for dot in dots{dot.backgroundColor=NSColor.white.withAlphaComponent(0.48).cgColor;dot.cornerRadius=0.9;dot.opacity=0.25;addSublayer(dot)}
    }
    required init?(coder:NSCoder){fatalError()}
    override init(layer:Any){let source=layer as? ChatRowLayer;value=source?.value ?? "";super.init(layer:layer)}
    override func layoutSublayers(){super.layoutSublayers();CATransaction.begin();CATransaction.setDisableActions(true);for(i,dot) in dots.enumerated(){dot.frame=CGRect(x:CGFloat(i)*3.4,y:4.4,width:1.8,height:1.8)};title.frame=CGRect(x:13,y:1,width:max(0,bounds.width-13),height:10);CATransaction.commit()}
    func setHovered(_ active:Bool){guard active != hovered else{return};hovered=active;title.removeAnimation(forKey:"marquee");let available=max(0,bounds.width-13),full=ceil((value as NSString).size(withAttributes:[.font:font]).width);CATransaction.begin();CATransaction.setDisableActions(true);title.setAffineTransform(.identity);title.truncationMode=active ? .none:.end;title.frame=CGRect(x:13,y:1,width:active ? max(available,full):available,height:10);CATransaction.commit();guard active,full>available else{return};let distance=full-available+3,a=CAKeyframeAnimation(keyPath:"transform.translation.x");a.values=[0,0,-distance,-distance,0];a.keyTimes=[0,0.12,0.48,0.72,1];a.duration=max(2.4,Double(distance)/11);a.repeatCount = .infinity;a.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);title.add(a,forKey:"marquee")}
    func setSelected(_ selected:Bool){title.foregroundColor=NSColor.white.withAlphaComponent(selected ? 0.9:0.42).cgColor}
    func setRunning(_ running:Bool){
        guard (dots.first?.animation(forKey:"chatHop") != nil) != running else{return}
        let now=CACurrentMediaTime()
        for(i,dot) in dots.enumerated(){dot.opacity=running ? 1:0.25;dot.removeAnimation(forKey:"chatHop");if running{let hop=CAKeyframeAnimation(keyPath:"transform.translation.y");hop.values=[0,0,1.7,0,0];hop.keyTimes=[0,0.16,0.34,0.52,1];hop.duration=1.18;hop.beginTime=now+Double(i)*0.14;hop.repeatCount = .infinity;hop.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);dot.add(hop,forKey:"chatHop")}}
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
    func closeContains(_ point:CGPoint)->Bool{close.frame.insetBy(dx:-3,dy:-3).contains(point)}
}

@MainActor final class MiniAppInterface {
    let layer=CALayer()
    private var renderingScale:CGFloat=NSScreen.main?.backingScaleFactor ?? 2
    func setRenderingScale(_ scale:CGFloat){renderingScale=scale;NotchSizing.updateTextResolution(layer,scale:scale)}
    var onSettings: ((String,String)->Void)?
    var onNavigate: ((String)->Void)?
    var onContextMode: ((Bool)->Void)?
    private var modelState: KaiState?
    private var lightweightContext=false
    private let settingsBar=CALayer()
    private var settingsHits=[(CGRect,String)]()
    var composerFrameChanged:((CGRect,Bool)->Void)?
    var conversationFrameChanged:((CGRect)->Void)?
    var onFilesVisibleChanged:((Bool)->Void)?
    var onProject:((String)->Void)?,onChat:((String)->Void)?,onNew:(()->Void)?,onNewInProject:(()->Void)?,onFolder:((String)->Void)?,onSend:(()->Void)?,onApprove:(()->Void)?,onReject:(()->Void)?,onInterrupt:(()->Void)?,onClose:(()->Void)?,onImagesChanged:(([NSImage])->Void)?
    private let left=CALayer(), projectToggle=CAShapeLayer(), preview=FilePreviewLayer(), main=CALayer(), right=CALayer(), input=CAGradientLayer(), imageStrip=CALayer(), command=CALayer(), send=CAGradientLayer(), sendShade=CAGradientLayer(), sendGlass=CAGradientLayer(), arrow=CAShapeLayer(), message=CATextLayer(), response=CAGradientLayer(), responseMask=CALayer(), typingDots=(0..<3).map{_ in CALayer()},pendingHighlight=CALayer(),pendingRibbon=CAGradientLayer()
    private let newRow=CATextLayer(), projectNewRow=CATextLayer(), standaloneTitle=CATextLayer(), activeTitle=CATextLayer(), actionBar=CALayer(), closeButton=CATextLayer()
    private var projectRows=[(CALayer,String)](),chatRows=[(ChatRowLayer,String)](),standaloneRows=[(ChatRowLayer,String)](),activeRows=[(ChatRowLayer,String)](),projects=[KaiProject](),chats=[KaiChat](),standaloneChats=[KaiChat](),selectedProject="",selectedThread:String?,fileEntries=[KaiFile](),fileRows=[FileRowLayer](),pathHits=[(CGRect,URL)](),filesOpen=false,previewOpen=false,previewFromFiles=false,projectsCollapsed=false,projectIntrudesComposer=false,projectOffset:CGFloat=0,fileOffset:CGFloat=0,approvalNeeded=false,turnActive=false
    private var fileGlyphColors=[NSColor.systemBlue.cgColor,NSColor.systemPurple.cgColor,NSColor.white.cgColor]
    private var actionHits=[(CGRect,String)]()
    private weak var hoveredAttachment:AttachmentLayer?
    private(set) var pendingRequestID:String?
    private let leftWidth:CGFloat=86, drawerWidth:CGFloat=78

    init() {
        layer.opacity=0; layer.addSublayer(left);layer.addSublayer(projectToggle);layer.addSublayer(preview); layer.addSublayer(main); layer.addSublayer(right);left.masksToBounds=true;right.masksToBounds=true;projectToggle.fillColor=nil;projectToggle.strokeColor=NSColor.white.withAlphaComponent(0.38).cgColor;projectToggle.lineWidth=1;projectToggle.lineCap = .round;projectToggle.lineJoin = .round
        configure(newRow,"+ new chat",8,.white.withAlphaComponent(0.65));left.addSublayer(newRow)
        configure(projectNewRow,"+ in project",7,.white.withAlphaComponent(0.58));left.addSublayer(projectNewRow)
        configure(standaloneTitle,"CHATS",7,.white.withAlphaComponent(0.42),.medium);left.addSublayer(standaloneTitle)
        configure(activeTitle,"ACTIVE",7,.white.withAlphaComponent(0.42),.medium);activeTitle.opacity=0;left.addSublayer(activeTitle)
        configure(message,"Choose a project and chat",9,.white.withAlphaComponent(0.68),.medium);main.addSublayer(message);configure(closeButton,"×",12,.white.withAlphaComponent(0.65));closeButton.alignmentMode = .center;main.addSublayer(closeButton);main.addSublayer(actionBar);main.addSublayer(settingsBar)
        input.cornerRadius=14; input.borderWidth=0.75; input.masksToBounds=true; layer.addSublayer(input);imageStrip.masksToBounds=true;layer.addSublayer(imageStrip)
        input.addSublayer(label("type or hold",8,.white.withAlphaComponent(0.46))); command.cornerRadius=6; command.borderWidth=0.7; command.addSublayer(label("command",7.4,.white.withAlphaComponent(0.74),.medium)); input.addSublayer(command); input.addSublayer(label("to speak",8,.white.withAlphaComponent(0.46)))
        send.type = .radial;send.startPoint=CGPoint(x:0.5,y:0.5);send.endPoint=CGPoint(x:1,y:0.5);send.locations=[0,0.52,1];send.cornerRadius=13; send.borderWidth=0.8; send.masksToBounds=true; input.addSublayer(send)
        sendShade.type = .radial;sendShade.colors=[NSColor.clear.cgColor,NSColor.black.withAlphaComponent(0.3).cgColor];sendShade.locations=[0,1];sendShade.startPoint=CGPoint(x:0.5,y:0.5);sendShade.endPoint=CGPoint(x:1,y:0.5);sendShade.cornerRadius=13;sendShade.masksToBounds=true;send.addSublayer(sendShade)
        sendGlass.type = .radial; sendGlass.cornerRadius=13; sendGlass.masksToBounds=true; sendGlass.colors=[NSColor.white.withAlphaComponent(0.18).cgColor,NSColor.white.withAlphaComponent(0.04).cgColor,NSColor.clear.cgColor]; sendGlass.locations=[0,0.42,1]; sendGlass.startPoint=CGPoint(x:0.3,y:0.76);sendGlass.endPoint=CGPoint(x:0.94,y:0.08);send.addSublayer(sendGlass)
        arrow.fillColor=nil;arrow.strokeColor=NSColor.white.withAlphaComponent(0.92).cgColor;arrow.lineWidth=1.15;arrow.lineCap = .round;arrow.lineJoin = .round;send.addSublayer(arrow)
        pendingHighlight.opacity=0;pendingHighlight.cornerRadius=4;pendingHighlight.masksToBounds=true;pendingHighlight.backgroundColor=NSColor.white.withAlphaComponent(0.12).cgColor;pendingHighlight.borderColor=NSColor.white.withAlphaComponent(0.28).cgColor;pendingHighlight.borderWidth=0.6;pendingRibbon.colors=[NSColor.clear.cgColor,NSColor.white.withAlphaComponent(0.5).cgColor,NSColor.clear.cgColor];pendingRibbon.startPoint=CGPoint(x:0,y:0.5);pendingRibbon.endPoint=CGPoint(x:1,y:0.5);pendingHighlight.addSublayer(pendingRibbon);layer.addSublayer(pendingHighlight)
        installFlow()
    }

    private func beginPending(_ frame:CGRect){
        pendingRequestID=UUID().uuidString
        CATransaction.begin();CATransaction.setDisableActions(true);pendingHighlight.removeAllAnimations();pendingHighlight.frame=frame.insetBy(dx:-2,dy:-2);pendingHighlight.opacity=1;pendingRibbon.frame=CGRect(x:-34,y:0,width:34,height:pendingHighlight.bounds.height);CATransaction.commit()
        let sweep=CABasicAnimation(keyPath:"position.x");sweep.fromValue = -17;sweep.toValue=pendingHighlight.bounds.width+17;sweep.duration=0.95;sweep.repeatCount = .infinity;sweep.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);pendingRibbon.add(sweep,forKey:"loadingSweep")
        CATransaction.flush()
    }
    func finishPending(requestID:String?){guard let requestID,requestID == pendingRequestID else{return};cancelPending()}
    func cancelPending(){pendingRequestID=nil;pendingRibbon.removeAnimation(forKey:"loadingSweep");let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=pendingHighlight.presentation()?.opacity ?? pendingHighlight.opacity;fade.toValue=0;fade.duration=0.18;CATransaction.begin();CATransaction.setDisableActions(true);pendingHighlight.opacity=0;CATransaction.commit();pendingHighlight.add(fade,forKey:"pendingFade")}

    func layout(in frame:CGRect) {
        layer.frame=frame; left.frame=CGRect(x:0,y:31,width:leftWidth,height:layer.bounds.height-31);projectToggle.frame=CGRect(x:0,y:2,width:12,height:16);layoutProjectToggle(animated:false)
        right.frame=CGRect(x:layer.bounds.width-drawerWidth,y:35,width:drawerWidth,height:layer.bounds.height-43);layoutProjects(animated:false); layoutMain(animated:false);layoutFiles()
        setFilesVisible(filesOpen,animated:false)
    }

    private func layoutProjects(animated:Bool) {
        var cursor=left.bounds.height-4+projectOffset
        setFrame(newRow,CGRect(x:5,y:cursor-12,width:leftWidth-10,height:11),animated:animated);cursor-=21
        projectNewRow.opacity=projectRows.contains(where:{$0.1 == selectedProject}) ? 1:0
        standaloneTitle.opacity=standaloneRows.isEmpty ? 0:1
        if !standaloneRows.isEmpty {
            setFrame(standaloneTitle,CGRect(x:5,y:cursor-10,width:leftWidth-10,height:9),animated:animated);cursor-=15
            for (chat,id) in standaloneRows {setFrame(chat,CGRect(x:9,y:cursor-13,width:leftWidth-13,height:12),animated:animated);chat.setSelected(id == selectedThread);cursor-=16}
            cursor-=6
        }
        activeTitle.opacity=activeRows.isEmpty ? 0:1
        if !activeRows.isEmpty {
            setFrame(activeTitle,CGRect(x:5,y:cursor-10,width:leftWidth-10,height:9),animated:animated);cursor-=15
            for (chat,id) in activeRows {setFrame(chat,CGRect(x:4,y:cursor-13,width:leftWidth-8,height:12),animated:animated);chat.setSelected(id == selectedThread);cursor-=16}
            cursor-=6
        }
        for (row,path) in projectRows {
            setFrame(row,CGRect(x:4,y:cursor-17,width:leftWidth-8,height:16),animated:animated);cursor-=21
            if path == selectedProject {
                for (chat,id) in chatRows { setFrame(chat,CGRect(x:18,y:cursor-13,width:leftWidth-21,height:12),animated:animated);chat.setSelected(id == selectedThread);cursor-=16 }
                setFrame(projectNewRow,CGRect(x:19,y:cursor-12,width:leftWidth-22,height:11),animated:animated);cursor-=19
            }
        }
        projectIntrudesComposer=cursor+left.frame.minY<36
    }

    private func layoutMain(animated:Bool) {
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
        let minX=projectIntrudesComposer && !projectsCollapsed && !previewOpen ? left.frame.maxX+9:18,maxX=paired ? fileX-4:(previewOpen ? layer.bounds.width-leftWidth-8:(filesOpen ? layer.bounds.width-drawerWidth-9:layer.bounds.width-18)),composerW=min(260,max(70,maxX-minX)),idealX=(minX+maxX-composerW)/2,inputX=min(max(idealX,minX),maxX-composerW),composerFrame=CGRect(x:inputX,y:0,width:composerW,height:28);setFrame(input,composerFrame,animated:animated);composerFrameChanged?(composerFrame,animated);setFrame(imageStrip,CGRect(x:inputX+8,y:34,width:max(0,composerW-16),height:32),animated:animated);layoutImages();let texts=input.sublayers?.compactMap{$0 as? CATextLayer} ?? []
        if composerW < 200 { texts.first?.string="hold";texts.first?.frame=CGRect(x:11,y:8,width:22,height:11);command.frame=CGRect(x:36,y:5,width:47,height:18);if texts.count>1{texts[1].string=""} }
        else { texts.first?.string="type or hold";texts.first?.frame=CGRect(x:11,y:8,width:55,height:11);command.frame=CGRect(x:68,y:5,width:47,height:18);if texts.count>1{texts[1].string="to speak";texts[1].frame=CGRect(x:120,y:8,width:43,height:11)} }
        command.sublayers?.first?.frame=CGRect(x:4,y:4,width:39,height:10)
        setFrame(send,CGRect(x:composerW-27,y:1,width:26,height:26),animated:animated);sendShade.frame=send.bounds;sendGlass.frame=send.bounds;let p=CGMutablePath();p.move(to:CGPoint(x:9,y:14.5));p.addLine(to:CGPoint(x:13,y:18.5));p.addLine(to:CGPoint(x:17,y:14.5));p.move(to:CGPoint(x:13,y:18.5));p.addLine(to:CGPoint(x:13,y:7.5));arrow.path=p;arrow.frame=send.bounds
        let conversationBottom:CGFloat=(imageStrip.sublayers?.isEmpty == false) ? 68:32
        conversationFrameChanged?(CGRect(x:target.minX+2,y:conversationBottom,width:max(0,target.width-4),height:max(0,h-56-conversationBottom)))
    }

    func handleClick(_ point:CGPoint)->Bool {
        let settingsPoint=CGPoint(x:point.x-main.frame.minX-settingsBar.frame.minX,y:point.y-main.frame.minY-settingsBar.frame.minY)
        if let hit=settingsHits.first(where:{$0.0.contains(settingsPoint)}) {
            if hit.1 == "context" {lightweightContext.toggle();onContextMode?(lightweightContext);layoutSettings()}
            else {showSettingsMenu(hit.1)}
            return true
        }
        let closePoint=CGPoint(x:point.x-main.frame.minX,y:point.y-main.frame.minY)
        if closeButton.frame.insetBy(dx:-3,dy:-2).contains(closePoint){onClose?();return true}
        if !previewOpen,projectToggle.frame.contains(point){toggleProjects();return true}
        if input.frame.contains(point) {
            let local=CGPoint(x:point.x-input.frame.minX,y:point.y-input.frame.minY)
            if send.frame.contains(local) { onSend?(); return true }
        }
        let actionPoint=CGPoint(x:point.x-main.frame.minX-actionBar.frame.minX,y:point.y-main.frame.minY-actionBar.frame.minY)
        if let action=actionHits.first(where:{$0.0.contains(actionPoint)}) {
            beginPending(action.0.offsetBy(dx:main.frame.minX+actionBar.frame.minX,dy:main.frame.minY+actionBar.frame.minY))
            switch action.1 { case "back":onNavigate?("back");case "forward":onNavigate?("forward");case "new":onNew?();case "approve":onApprove?();case "reject":onReject?();case "interrupt":onInterrupt?();default:cancelPending() };return true
        }
        let attachmentPoint=CGPoint(x:point.x-imageStrip.frame.minX,y:point.y-imageStrip.frame.minY)
        if imageStrip.frame.contains(point),let preview=((hoveredAttachment?.frame.contains(attachmentPoint) == true ? hoveredAttachment:nil) ?? imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer}.reversed().first{$0.frame.contains(attachmentPoint)}) { let local=CGPoint(x:attachmentPoint.x-preview.frame.minX,y:attachmentPoint.y-preview.frame.minY);if preview.closeContains(local){remove(preview)};return true }
        let previewPoint=CGPoint(x:point.x-preview.frame.minX,y:point.y-preview.frame.minY);if previewOpen,preview.frame.contains(point),preview.closeContains(previewPoint){setPreviewVisible(false);return true}
        let rightPoint=CGPoint(x:point.x-right.frame.minX,y:point.y-right.frame.minY);if filesOpen,right.frame.contains(point),let row=fileRows.first(where:{$0.frame.insetBy(dx:0,dy:-3).contains(rightPoint)}){beginPending(row.frame.offsetBy(dx:right.frame.minX,dy:right.frame.minY));if row.isDirectory {onFolder?(row.url.path)}else{openPreview(row.url,pendingID:pendingRequestID)};return true}
        let mainPoint=CGPoint(x:point.x-main.frame.minX,y:point.y-main.frame.minY);if let hit=pathHits.first(where:{$0.0.contains(mainPoint)}){openPreview(hit.1);return true}
        let local=CGPoint(x:point.x-left.frame.minX,y:point.y-left.frame.minY)
        if !projectsCollapsed && !previewOpen {
            if newRow.frame.contains(local){beginPending(newRow.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY));onNew?();return true}
            if let chat=standaloneRows.first(where:{$0.0.frame.contains(local)}){beginPending(chat.0.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY));onChat?(chat.1);return true}
            if let chat=activeRows.first(where:{$0.0.frame.contains(local)}){beginPending(chat.0.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY));onChat?(chat.1);return true}
            if let project=projectRows.first(where:{$0.0.frame.contains(local)}){beginPending(project.0.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY));onProject?(project.1);return true}
            if let chat=chatRows.first(where:{$0.0.frame.contains(local)}){beginPending(chat.0.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY));onChat?(chat.1);return true}
            if projectRows.contains(where:{$0.1 == selectedProject}) && projectNewRow.frame.contains(local){beginPending(projectNewRow.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY));onNewInProject?();return true}
        }
        return layer.bounds.contains(point)
    }

    func handleHover(_ point:CGPoint?) {
        guard let point else { (chatRows+standaloneRows+activeRows).forEach{$0.0.setHovered(false)};hoveredAttachment=nil;for(i,item) in (imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer} ?? []).enumerated(){item.setHovered(false);item.zPosition=CGFloat(i)};return }
        let attachmentPoint=CGPoint(x:point.x-imageStrip.frame.minX,y:point.y-imageStrip.frame.minY),previews=imageStrip.sublayers?.compactMap{$0 as? AttachmentLayer} ?? [],hovered=(hoveredAttachment?.frame.contains(attachmentPoint) == true ? hoveredAttachment:previews.reversed().first{$0.frame.contains(attachmentPoint)})
        hoveredAttachment=hovered;for(i,item) in previews.enumerated(){item.setHovered(item === hovered);item.zPosition=item === hovered ? 1000:CGFloat(i)}
        let p=CGPoint(x:point.x-left.frame.minX,y:point.y-left.frame.minY)
        for (row,_) in chatRows { row.setHovered(row.frame.contains(p)) }
        for (row,_) in standaloneRows { row.setHovered(row.frame.contains(p)) }
        for (row,_) in activeRows { row.setHovered(row.frame.contains(p)) }
    }

    func scroll(_ delta:CGFloat,at point:CGPoint) { if filesOpen,right.frame.contains(point){let overflow=max(0,CGFloat(fileEntries.count)*18-right.bounds.height+8);fileOffset=min(overflow,max(0,fileOffset-delta));layoutFiles()}else if left.frame.contains(point){let total=CGFloat(projectRows.count)*21+CGFloat(chatRows.count)*16+CGFloat(standaloneRows.isEmpty ? 0:21+standaloneRows.count*16)+CGFloat(activeRows.isEmpty ? 0:21+activeRows.count*16)+40;projectOffset=min(max(0,total-left.bounds.height+8),max(0,projectOffset-delta));layoutProjects(animated:false)} }

    func toggleFiles() { if previewOpen && !previewFromFiles{setPreviewVisible(false)};filesOpen.toggle();if !filesOpen{previewFromFiles=false};setFilesVisible(filesOpen,animated:true);onFilesVisibleChanged?(filesOpen);layoutMain(animated:true) }
    var isFilesOpen:Bool { filesOpen }
    func resetFiles() { if previewOpen{setPreviewVisible(false)};filesOpen=false;fileOffset=0;setFilesVisible(false,animated:false);onFilesVisibleChanged?(false);layoutFiles();layoutMain(animated:false) }

    func setCodexMessage(_ text:String){let font=NSFont.systemFont(ofSize:8),result=NSMutableAttributedString(string:text,attributes:[.font:font,.foregroundColor:NSColor.white.withAlphaComponent(0.62)]),pattern=#"(?:file://)?(?:/Users/|Users/|~/)[^\s\]\[\)\(>,]+"#;pathHits.removeAll();if let regex=try? NSRegularExpression(pattern:pattern){for match in regex.matches(in:text,range:NSRange(text.startIndex...,in:text)){result.addAttributes([.font:NSFont.systemFont(ofSize:8,weight:.bold),.underlineStyle:NSUnderlineStyle.single.rawValue,.foregroundColor:NSColor.white.withAlphaComponent(0.9)],range:match.range)}};message.string=result;layoutPathHits()}
    func setState(_ state:KaiState){
        modelState=state
        selectedProject=state.project;selectedThread=state.threadId;approvalNeeded=state.approval;turnActive=state.activeTurnId != nil
        if projects.map(\.path) != state.projects.map(\.path){projectRows.forEach{$0.0.removeFromSuperlayer()};projectRows.removeAll();projects=state.projects;for project in projects{let row=CALayer(),folder=FileGlyphLayer(size:CGSize(width:16,height:13)),title=label(project.name,8,.white.withAlphaComponent(0.72));row.addSublayer(folder);row.addSublayer(title);folder.frame=CGRect(x:0,y:1,width:16,height:13);title.frame=CGRect(x:19,y:2,width:leftWidth-30,height:11);left.addSublayer(row);projectRows.append((row,project.path))}}
        if chats.map(\.id) != state.chats.map(\.id) || chats.map(\.name) != state.chats.map(\.name){chatRows.forEach{$0.0.removeFromSuperlayer()};chatRows.removeAll();chats=state.chats;for chat in chats{let row=ChatRowLayer(chat.name);left.addSublayer(row);chatRows.append((row,chat.id))}}
        if standaloneChats.map(\.id) != state.standaloneChats.map(\.id) || standaloneChats.map(\.name) != state.standaloneChats.map(\.name){standaloneRows.forEach{$0.0.removeFromSuperlayer()};standaloneRows.removeAll();standaloneChats=state.standaloneChats;for chat in standaloneChats{let row=ChatRowLayer(chat.name);left.addSublayer(row);standaloneRows.append((row,chat.id))}}
        for(row,id) in chatRows{row.setRunning(state.chats.first(where:{$0.id == id})?.status == "active")}
        for(row,id) in standaloneRows{row.setRunning(state.standaloneChats.first(where:{$0.id == id})?.status == "active")}
        let visibleIDs=Set(state.chats.map(\.id)+state.standaloneChats.map(\.id)),elsewhere=state.activeTasks.filter{!visibleIDs.contains($0.id)}
        if activeRows.map({$0.1}) != elsewhere.map(\.id) || activeRows.map({$0.0.name ?? ""}) != elsewhere.map(\.name){
            if activeRows.isEmpty && !elsewhere.isEmpty{projectOffset=0}
            activeRows.forEach{$0.0.removeFromSuperlayer()};activeRows.removeAll()
            for task in elsewhere {let row=ChatRowLayer(task.name);row.name=task.name;row.setRunning(true);left.addSublayer(row);activeRows.append((row,task.id))}
        }
        if let files=state.files {
            if fileEntries.count != files.count || zip(fileEntries,files).contains(where:{$0.path != $1.path || $0.collapsed != $1.collapsed}) {
                fileEntries=files;fileOffset=min(fileOffset,max(0,CGFloat(fileEntries.count)*18-right.bounds.height+8));layoutFiles()
            }
        }
        let title=state.threadId == nil ? "Choose a chat" : (state.threadName.isEmpty ? "Chat" : state.threadName)
        let focus=state.activeTasks.first(where:{$0.id == state.focusThreadId})
        let percent=state.rateLimitRemainingPercent.map{" · \($0)%"} ?? ""
        let name=focus?.name ?? title
        let activityText=state.approval ? "Approval needed" : (focus?.text ?? (state.activity.scene == "off" ? "" : state.activity.text))
        let shownReadOnly=state.readOnly && (focus == nil || focus?.id == state.threadId)
        setCodexMessage(name+percent+(activityText.isEmpty ? "":" · "+activityText)+(shownReadOnly ? " · read only":""))
        message.truncationMode = .middle
        layoutProjects(animated:false);layoutMain(animated:false);NotchSizing.updateTextResolution(layer,scale:renderingScale)
    }
    func setContextMode(_ lightweight:Bool){lightweightContext=lightweight;layoutSettings()}
    private func layoutSettings() {
        settingsBar.sublayers?.forEach{$0.removeFromSuperlayer()};settingsHits.removeAll()
        guard let state=modelState else{return}
        let model=state.models?.first{$0.model == state.selectedModel}
        let name=model?.displayName ?? state.selectedModel ?? "Models unavailable"
        let effort=state.reasoningEffort ?? "Effort"
        var x:CGFloat=0
        for (id,text) in [("model",name),("effort",effort),("context",lightweightContext ? "Context: light":"Context: screen")] {
            let width=min(id == "model" ? 100: id == "effort" ? 48:80, min(CGFloat(text.count)*4.5+9,max(0,settingsBar.bounds.width-x)))
            guard width>16 else{break}
            let mask=label(text,7.5,.white,.medium);mask.frame=CGRect(x:0,y:1,width:width,height:11)
            let gradient=CAGradientLayer();gradient.frame=CGRect(x:x,y:0,width:width,height:13);gradient.startPoint=CGPoint(x:0,y:0);gradient.endPoint=CGPoint(x:1,y:1);gradient.mask=mask
            let lower=name.lowercased()
            let colors:[NSColor]=lower.contains("astra") ? [.systemPurple,.systemBlue,.systemPink]:lower.contains("luna") ? [.gray,.white,.lightGray]:lower.contains("terra") ? [.systemBlue,.systemGreen,.systemTeal]:lower.contains("sol") ? [.systemYellow,.systemOrange,.systemRed]:[.systemTeal,.systemBlue,.systemPurple]
            let levels=["none","minimal","low","medium","high","xhigh","max","ultra"]
            let intensity=CGFloat(levels.firstIndex(of:effort) ?? 3)/7
            gradient.colors=(id == "effort" ? [NSColor.white.withAlphaComponent(0.45+intensity*0.55),NSColor.systemPurple.withAlphaComponent(0.4+intensity*0.6),NSColor.systemPink.withAlphaComponent(0.4+intensity*0.6)]:colors).map(\.cgColor)
            settingsBar.addSublayer(gradient);settingsHits.append((gradient.frame,id));x+=width+6
        }
    }
    private func showSettingsMenu(_ kind:String) {
        guard let state=modelState,let models=state.models else{return}
        let menu=NSMenu()
        if kind == "model" {
            for model in models {
                let item=menu.addItem(withTitle:model.displayName,action:#selector(selectSetting(_:)),keyEquivalent:"");item.target=self;item.representedObject=[model.model,state.reasoningEffort ?? ""];item.state=model.model == state.selectedModel ? .on:.off
            }
        } else if let model=models.first(where:{$0.model == state.selectedModel}) {
            for effort in model.supportedReasoningEfforts {
                let item=menu.addItem(withTitle:effort.reasoningEffort,action:#selector(selectSetting(_:)),keyEquivalent:"");item.target=self;item.representedObject=[model.model,effort.reasoningEffort];item.state=effort.reasoningEffort == state.reasoningEffort ? .on:.off
            }
        }
        menu.popUp(positioning:nil,at:NSEvent.mouseLocation,in:nil)
    }
    @objc private func selectSetting(_ item:NSMenuItem) {guard let values=item.representedObject as? [String],values.count == 2 else{return};onSettings?(values[0],values[1])}

    private func layoutActions(){actionBar.sublayers?.forEach{$0.removeFromSuperlayer()};actionHits.removeAll();var x:CGFloat=0
        let actions:[(String,String)]=((modelState?.canGoBack == true ? [("back","‹")]:[])+(modelState?.canGoForward == true ? [("forward","›")]:[]))+(approvalNeeded ? [("approve","Approve"),("reject","Reject")]:[])+(turnActive ? [("interrupt","Stop")]:[])+[("new","+ New")]
        for (id,title) in actions {let width=CGFloat(title.count)*5+10;if x+width>actionBar.bounds.width{break};let item=label(title,7.5,.white.withAlphaComponent(0.74),.medium);item.frame=CGRect(x:x+4,y:1,width:width-8,height:11);actionBar.addSublayer(item);actionHits.append((CGRect(x:x,y:0,width:width,height:13),id));x+=width+3}
    }
    func previewFile(_ url:URL){openPreview(url)}

    func setComposerHasText(_ hasText:Bool){CATransaction.begin();CATransaction.setDisableActions(true);for item in input.sublayers ?? [] where item !== send{item.opacity=hasText ? 0:1};CATransaction.commit()}
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

    private func layoutFiles(){fileRows.forEach{$0.removeFromSuperlayer()};fileRows.removeAll();let first=max(0,Int(fileOffset/18)-1),last=min(fileEntries.count,first+Int(right.bounds.height/18)+4);guard first<last else{return};CATransaction.begin();CATransaction.setDisableActions(true);for i in first..<last {let entry=fileEntries[i],indent=CGFloat(min(entry.depth,8))*5,row=FileRowLayer(URL(fileURLWithPath:entry.path),isDirectory:entry.directory,open:entry.directory && !entry.collapsed);row.string=entry.name;row.frame=CGRect(x:3+indent,y:right.bounds.height-15+fileOffset-CGFloat(i)*18,width:max(24,drawerWidth-6-indent),height:14);row.setColors(fileGlyphColors,animated:false);right.addSublayer(row);fileRows.append(row)};CATransaction.commit()}

    private func openPreview(_ url:URL,pendingID:String?=nil){previewFromFiles=filesOpen;preview.show(url){[weak self] in if let pendingID{self?.finishPending(requestID:pendingID)}};if previewOpen{layoutMain(animated:true)}else{setPreviewVisible(true)}}
    private func setPreviewVisible(_ visible:Bool){guard visible != previewOpen else{return};previewOpen=visible;if !visible{previewFromFiles=false};let duration=0.46,curve=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);func transition(_ item:CALayer,opacity:Float,from fallback:CGFloat,to offset:CGFloat){let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=item.presentation()?.opacity ?? item.opacity;fade.toValue=opacity;fade.duration=duration;fade.timingFunction=curve;let slide=CABasicAnimation(keyPath:"transform.translation.x");slide.fromValue=item.presentation()?.transform.m41 ?? fallback;slide.toValue=offset;slide.duration=duration;slide.timingFunction=curve;CATransaction.begin();CATransaction.setDisableActions(true);item.opacity=opacity;item.setAffineTransform(CGAffineTransform(translationX:offset,y:0));CATransaction.commit();item.add(fade,forKey:"previewFade");item.add(slide,forKey:"previewSlide")};transition(left,opacity:visible || projectsCollapsed ? 0:1,from:visible ? 0:-16,to:visible || projectsCollapsed ? -16:0);transition(preview,opacity:visible ? 1:0,from:visible ? leftWidth*0.62:0,to:visible ? 0:leftWidth*0.62);projectToggle.opacity=visible ? 0:1;layoutProjectToggle(animated:true);layoutMain(animated:true)}
    private func toggleProjects(){projectsCollapsed.toggle();let target:Float=projectsCollapsed ? 0:1,offset:CGFloat=projectsCollapsed ? -16:0,fade=CABasicAnimation(keyPath:"opacity"),slide=CABasicAnimation(keyPath:"transform.translation.x"),curve=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);fade.fromValue=left.presentation()?.opacity ?? left.opacity;fade.toValue=target;fade.duration=0.42;fade.timingFunction=curve;slide.fromValue=left.presentation()?.transform.m41 ?? left.transform.m41;slide.toValue=offset;slide.duration=0.42;slide.timingFunction=curve;CATransaction.begin();CATransaction.setDisableActions(true);left.opacity=target;left.setAffineTransform(CGAffineTransform(translationX:offset,y:0));CATransaction.commit();left.add(fade,forKey:"projectFade");left.add(slide,forKey:"projectSlide");layoutProjectToggle(animated:true);layoutMain(animated:true)}
    private func layoutProjectToggle(animated:Bool){let right=projectsCollapsed,p=CGMutablePath();p.move(to:CGPoint(x:right ? 4:8,y:4));p.addLine(to:CGPoint(x:right ? 8:4,y:8));p.addLine(to:CGPoint(x:right ? 4:8,y:12));let old=projectToggle.presentation()?.path ?? projectToggle.path;CATransaction.begin();CATransaction.setDisableActions(true);projectToggle.path=p;CATransaction.commit();if animated{let morph=CABasicAnimation(keyPath:"path");morph.fromValue=old;morph.toValue=p;morph.duration=0.3;morph.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);projectToggle.add(morph,forKey:"direction")}}
    private func layoutPathHits(){pathHits.removeAll();guard let attributed=message.string as? NSAttributedString else{return};let text=attributed.string,font=NSFont.systemFont(ofSize:8),pattern=#"(?:file://)?(?:/Users/|Users/|~/)[^\s\]\[\)\(>,]+"#;guard let regex=try? NSRegularExpression(pattern:pattern) else{return};for match in regex.matches(in:text,range:NSRange(text.startIndex...,in:text)){let before=(text as NSString).substring(to:match.range.location),path=(text as NSString).substring(with:match.range),x=(before as NSString).size(withAttributes:[.font:font]).width,w=(path as NSString).size(withAttributes:[.font:NSFont.systemFont(ofSize:8,weight:.bold)]).width;let url:URL?;if path.hasPrefix("file://"){url=URL(string:path)}else{url=URL(fileURLWithPath:path.hasPrefix("Users/") ? "/"+path:(path as NSString).expandingTildeInPath)};if let url{pathHits.append((CGRect(x:message.frame.minX+x,y:message.frame.minY,width:w,height:13),url))}}}

    private func collapse(_ group:CALayer,_ expanded:Bool) { let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=group.presentation()?.opacity ?? group.opacity;fade.toValue=expanded ? 1:0;fade.duration=0.26;fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);group.opacity=expanded ? 1:0;group.setAffineTransform(.identity);CATransaction.commit();group.add(fade,forKey:"collapse") }

    func setVisible(_ visible:Bool,duration:Double) { let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=layer.presentation()?.opacity ?? layer.opacity;fade.toValue=visible ? 1:0;fade.duration=duration*0.72;fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);layer.opacity=visible ? 1:0;CATransaction.commit();layer.add(fade,forKey:"visibility") }

    func setProfile(_ profile:PetGradientProfile,animated:Bool) {
        let s=profile.gradients.ambient.stops,c=[s.first!,s[s.count/2],s.last!].map{color($0.color)},radial=c.sorted{brightness($0)<brightness($1)};fileGlyphColors=c.map{readable($0)};fileRows.forEach{$0.setColors(fileGlyphColors,animated:animated)};animate(input,to:[alpha(c[0],0.15),alpha(c[1],0.065),alpha(c[2],0.1)],animated ? 0.72:0);animate(response,to:c.map{alpha(readable($0),0.92)},animated ? 0.72:0);animate(send,to:[mix(radial[2],NSColor.white.cgColor,0.2),mix(radial[1],NSColor.black.cgColor,0.16),mix(radial[0],NSColor.black.cgColor,0.58)],animated ? 0.72:0)
        CATransaction.begin();CATransaction.setDisableActions(true);input.borderColor=alpha(c[2],0.22);command.backgroundColor=alpha(c[0],0.14);command.borderColor=alpha(c[2],0.22);send.borderColor=NSColor.white.withAlphaComponent(0.36).cgColor;CATransaction.commit()
    }

    private func setFrame(_ item:CALayer,_ target:CGRect,animated:Bool) { let old=item.presentation()?.frame ?? item.frame;CATransaction.begin();CATransaction.setDisableActions(true);item.frame=target;CATransaction.commit();guard animated else{return};let pos=CABasicAnimation(keyPath:"position");pos.fromValue=NSValue(point:NSPoint(x:old.midX,y:old.midY));pos.toValue=NSValue(point:NSPoint(x:target.midX,y:target.midY));pos.duration=0.42;pos.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);item.add(pos,forKey:"position");if old.size != target.size { let bounds=CABasicAnimation(keyPath:"bounds");bounds.fromValue=NSValue(rect:NSRect(origin:.zero,size:old.size));bounds.toValue=NSValue(rect:NSRect(origin:.zero,size:target.size));bounds.duration=0.42;bounds.timingFunction=pos.timingFunction;item.add(bounds,forKey:"bounds") } }
    private func label(_ value:String,_ size:CGFloat,_ color:NSColor,_ weight:NSFont.Weight = .regular)->CATextLayer { let t=CATextLayer();configure(t,value,size,color,weight);return t }
    private func configure(_ t:CATextLayer,_ value:String,_ size:CGFloat,_ color:NSColor,_ weight:NSFont.Weight = .regular) { t.string=value;t.font=NSFont.systemFont(ofSize:size,weight:weight);t.fontSize=size;t.foregroundColor=color.cgColor;t.contentsScale=renderingScale;t.truncationMode = .end }
    private func installFlow(){response.startPoint=CGPoint(x:0,y:0.5);response.endPoint=CGPoint(x:1,y:0.5);response.locations=[0,0.45,1];let a=CABasicAnimation(keyPath:"locations");a.fromValue=[-0.12,0.28,0.88];a.toValue=[0.12,0.72,1.12];a.duration=2.8;a.autoreverses=true;a.repeatCount = .infinity;a.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);response.add(a,forKey:"flow")}
    private func installTyping(){let now=CACurrentMediaTime();for(i,dot) in typingDots.enumerated(){dot.frame=CGRect(x:CGFloat(i)*5.8+1,y:3.4,width:2.6,height:2.6);dot.cornerRadius=1.3;dot.backgroundColor=NSColor.white.cgColor;let jump=CAKeyframeAnimation(keyPath:"transform.translation.y");jump.values=[0,0,2.4,0,0];jump.keyTimes=[0,0.16,0.34,0.52,1];jump.duration=1.18;jump.beginTime=now+Double(i)*0.14;jump.repeatCount = .infinity;jump.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);dot.add(jump,forKey:"typing")}}
    private func animate(_ g:CAGradientLayer,to target:[CGColor],_ duration:Double){let from=(g.presentation()?.colors as? [CGColor]) ?? (g.colors as? [CGColor]) ?? target;CATransaction.begin();CATransaction.setDisableActions(true);g.colors=target;CATransaction.commit();guard duration>0 else{return};let a=CAKeyframeAnimation(keyPath:"colors");a.values=(0..<18).map{i in zip(from,target).map{mix($0.0,$0.1,CGFloat(i)/17)}};a.duration=duration;a.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);g.add(a,forKey:"theme")}
    private func mix(_ a:CGColor,_ b:CGColor,_ t:CGFloat)->CGColor{let x=NSColor(cgColor:a)?.usingColorSpace(.sRGB) ?? .clear,y=NSColor(cgColor:b)?.usingColorSpace(.sRGB) ?? .clear;var ah:CGFloat=0,asat:CGFloat=0,ab:CGFloat=0,aa:CGFloat=0,bh:CGFloat=0,bs:CGFloat=0,bb:CGFloat=0,ba:CGFloat=0;x.getHue(&ah,saturation:&asat,brightness:&ab,alpha:&aa);y.getHue(&bh,saturation:&bs,brightness:&bb,alpha:&ba);var d=bh-ah;if d>0.5{d-=1};if d < -0.5{d+=1};let h=(ah+d*t).truncatingRemainder(dividingBy:1);return NSColor(calibratedHue:h<0 ? h+1:h,saturation:asat+(bs-asat)*t,brightness:ab+(bb-ab)*t,alpha:aa+(ba-aa)*t).cgColor}
    private func color(_ hex:String)->CGColor{let v=UInt64(hex.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((v>>16)&255)/255,green:CGFloat((v>>8)&255)/255,blue:CGFloat(v&255)/255,alpha:1).cgColor}
    private func brightness(_ c:CGColor)->CGFloat{let x=NSColor(cgColor:c)?.usingColorSpace(.sRGB) ?? .black;var h:CGFloat=0,s:CGFloat=0,b:CGFloat=0,a:CGFloat=0;x.getHue(&h,saturation:&s,brightness:&b,alpha:&a);return b}
    private func readable(_ c:CGColor)->CGColor{let x=NSColor(cgColor:c)?.usingColorSpace(.sRGB) ?? .white;var h:CGFloat=0,s:CGFloat=0,b:CGFloat=0,a:CGFloat=0;x.getHue(&h,saturation:&s,brightness:&b,alpha:&a);return NSColor(calibratedHue:h,saturation:min(s,0.82),brightness:max(b,0.74),alpha:1).cgColor}
    private func alpha(_ c:CGColor,_ a:CGFloat)->CGColor{(NSColor(cgColor:c) ?? .clear).withAlphaComponent(a).cgColor}
}
