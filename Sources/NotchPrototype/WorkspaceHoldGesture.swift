import Foundation

/// One continuous Control–Option hold: normal threshold opens the mini chat;
/// continuing the same hold transforms it into the workspace. Release cancels.
struct WorkspaceHoldGesture {
    enum Action:Equatable {case mini,workspace,fold}
    static let miniDelay:TimeInterval=1
    static let workspaceDelay:TimeInterval=1.8
    private(set) var active=false
    private var started:TimeInterval=0,fromWorkspace=false,miniSent=false,longSent=false
    mutating func begin(at time:TimeInterval,workspace:Bool){active=true;started=time;fromWorkspace=workspace;miniSent=false;longSent=false}
    mutating func cancel(){active=false}
    mutating func advance(at time:TimeInterval)->Action?{
        guard active else{return nil};let elapsed=time-started
        if fromWorkspace {
            guard elapsed>=Self.workspaceDelay,!longSent else{return nil};longSent=true;return .fold
        }
        if elapsed>=Self.workspaceDelay,!longSent{longSent=true;miniSent=true;return .workspace}
        if elapsed>=Self.miniDelay,!miniSent{miniSent=true;return .mini}
        return nil
    }
}
