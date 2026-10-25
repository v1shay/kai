import Foundation

/// One continuous Control–Option hold: normal threshold opens the mini chat;
/// continuing the same hold transforms it into the workspace. Release cancels.
struct WorkspaceHoldGesture {
    enum Action:Equatable {case mini,workspace,fold}
    static let miniDelay:TimeInterval=1
    static let workspaceDelay:TimeInterval=1.8
    private(set) var active=false
    private var started:TimeInterval=0,fromWorkspace=false,fromOverlay=false,miniSent=false,longSent=false
    mutating func begin(at time:TimeInterval,workspace:Bool,overlay:Bool=false){active=true;started=time;fromWorkspace=workspace;fromOverlay=overlay;miniSent=false;longSent=false}
    mutating func cancel(){active=false}
    mutating func advance(at time:TimeInterval)->Action?{
        guard active else{return nil};let elapsed=time-started
        if fromOverlay{guard elapsed>=Self.miniDelay,!longSent else{return nil};longSent=true;miniSent=true;return .workspace}
        if fromWorkspace {
            guard elapsed>=Self.workspaceDelay,!longSent else{return nil};longSent=true;return .fold
        }
        if elapsed>=Self.workspaceDelay,!longSent{longSent=true;miniSent=true;return .workspace}
        if elapsed>=Self.miniDelay,!miniSent{miniSent=true;return .mini}
        return nil
    }
}
