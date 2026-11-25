import AppKit

/// Code's already-processed pixels, with a native bottom-to-top feather.
/// No file decoding, effect renderer, web view, or animation timer lives here.
@MainActor final class NotchWorkspaceBackground:NSView {
    private let artwork=CALayer(),feather=CAGradientLayer()
    override init(frame:NSRect){
        super.init(frame:frame);wantsLayer=true;layer?.addSublayer(artwork)
        artwork.contentsGravity = .resizeAspectFill;artwork.masksToBounds=true
        feather.colors=[NSColor.white.cgColor,NSColor.white.withAlphaComponent(0.92).cgColor,NSColor.white.withAlphaComponent(0.32).cgColor,NSColor.clear.cgColor]
        feather.locations=[0,0.20,0.62,1];feather.startPoint=CGPoint(x:0.5,y:0);feather.endPoint=CGPoint(x:0.5,y:1);artwork.mask=feather;isHidden=true
    }
    required init?(coder:NSCoder){fatalError("init(coder:) has not been implemented")}
    override func hitTest(_ point:NSPoint)->NSView?{nil}
    func adaptiveComposite(_ point:CGPoint,base:[Double])->[Double]{
        guard !isHidden,artwork.frame.contains(point),let image=AdaptiveColorMath.cgImage(artwork.contents),let data=image.dataProvider?.data,let bytes=CFDataGetBytePtr(data),image.bitsPerPixel==32,artwork.frame.width>0,artwork.frame.height>0 else{return base}
        let scale=max(artwork.frame.width/CGFloat(image.width),artwork.frame.height/CGFloat(image.height))
        let x=min(image.width-1,max(0,Int((point.x-artwork.frame.minX+(CGFloat(image.width)*scale-artwork.frame.width)*0.5)/scale)))
        let y=min(image.height-1,max(0,Int((artwork.frame.maxY-point.y+(CGFloat(image.height)*scale-artwork.frame.height)*0.5)/scale)))
        let at=y*image.bytesPerRow+x*4
        guard at+3<CFDataGetLength(data) else{return base}
        let alpha=Double(bytes[at+3])/255
        guard alpha>0 else{return base}
        let colors=feather.colors as? [CGColor] ?? []
        let t=(point.y-artwork.frame.minY)/artwork.frame.height
        let locations=feather.locations?.map(\.doubleValue) ?? [0,0.2,0.62,1]
        let high=locations.firstIndex(where:{$0>=t}) ?? locations.count-1,low=max(0,high-1)
        var mask=1.0
        if colors.count==locations.count,let a=AdaptiveColorMath.rgb(colors[low]),let b=AdaptiveColorMath.rgb(colors[high]){let f=locations[high]>locations[low] ? (t-locations[low])/(locations[high]-locations[low]):0;mask=a[3]+(b[3]-a[3])*f}
        let divisor=(image.alphaInfo == .premultipliedLast || image.alphaInfo == .premultipliedFirst) ? alpha:1
        return AdaptiveColorMath.over([Double(bytes[at])/255/divisor,Double(bytes[at+1])/255/divisor,Double(bytes[at+2])/255/divisor,alpha*Double(artwork.opacity)*mask],base)
    }
    var hasImage:Bool{artwork.contents != nil}
    func setImage(_ image:CGImage?,opacity:Float){
        CATransaction.begin();CATransaction.setDisableActions(true);artwork.contents=image;artwork.opacity=max(0,min(1,opacity));CATransaction.commit()
    }
    func update(canvas:CGSize,width:CGFloat,enabled:Bool,height:Double){
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        frame=CGRect(origin:.zero,size:canvas);isHidden = !enabled
        artwork.frame=CGRect(x:(canvas.width-width)/2,y:0,width:width,height:canvas.height*CGFloat(height))
        feather.frame=artwork.bounds
    }
}
