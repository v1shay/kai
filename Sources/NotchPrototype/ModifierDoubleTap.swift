import Foundation

/// Two bare modifier presses, with a release between them. Other key input and
/// chords cancel the sequence so ordinary editing cannot dismiss a surface.
struct ModifierDoubleTap {
    private var down=false,lastPress:TimeInterval?
    mutating func cancel(){down=false;lastPress=nil}
    mutating func update(down:Bool,eligible:Bool,at time:TimeInterval)->Bool{
        guard eligible else{cancel();self.down=down;return false}
        defer{self.down=down}
        guard down,!self.down else{return false}
        if let lastPress,time-lastPress>=0,time-lastPress<0.36{self.lastPress=nil;return true}
        lastPress=time;return false
    }
}
