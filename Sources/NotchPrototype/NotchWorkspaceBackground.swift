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
