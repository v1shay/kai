import AppKit

/// Geometry only. The conversation stays in the same NSWindow/GPUI view.
enum ConversationForm:String {
    case workspace,notch,mini,menu
    func frame(in screen:CGRect,workspace:CGRect?,anchor:CGRect?)->CGRect {
        if self == .workspace {return workspace.map{Self.fit($0,in:screen)} ?? screen}
        let width=min(self == .menu ? 560.0:680.0,screen.width-16)
        let height=min(self == .notch ? 560.0:540.0,screen.height-16)
        let center=self == .menu ? (anchor?.midX ?? screen.midX):screen.midX
        let top=self == .mini ? screen.midY+height/2:self == .menu ? (anchor?.minY ?? screen.maxY)-6:screen.maxY
        return Self.fit(CGRect(x:center-width/2,y:top-height,width:width,height:height),in:screen)
    }
    private static func fit(_ rect:CGRect,in screen:CGRect)->CGRect {
        let width=min(rect.width,screen.width),height=min(rect.height,screen.height)
        return CGRect(x:max(screen.minX,min(rect.minX,screen.maxX-width)),y:max(screen.minY,min(rect.minY,screen.maxY-height)),width:width,height:height)
    }
}
