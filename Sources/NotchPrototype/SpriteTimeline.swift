import QuartzCore

/// Discrete sprite frames run in the compositor, without a main-thread timer.
struct SpriteTimeline {
    let frames:[CGRect]
    let keyTimes:[NSNumber]
    let duration:Double
    let loops:Bool

    init?(row:Int,rows:Int,columns:[Int],durationsMs:[Int],loops:Bool) {
        guard rows>0,(0..<rows).contains(row),!columns.isEmpty,
              columns.count == durationsMs.count,columns.allSatisfy({(0..<8).contains($0)}) else{return nil}
        let durations=durationsMs.map{Double(max(16,$0))/1000}
        let total=durations.reduce(0,+)
        duration=total;self.loops=loops
        frames=columns.map{CGRect(x:CGFloat($0)/8,y:1-CGFloat(row+1)/CGFloat(rows),width:1/8,height:1/CGFloat(rows))}
        var elapsed=0.0
        keyTimes=durations.map{value in defer{elapsed += value};return NSNumber(value:elapsed/total)}+[1]
    }

    func animation()->CAKeyframeAnimation {
        let animation=CAKeyframeAnimation(keyPath:"contentsRect")
        animation.values=(frames+[loops ? frames[0]:frames[frames.count-1]]).map{NSValue(rect:$0)}
        animation.keyTimes=keyTimes;animation.calculationMode = .discrete
        animation.duration=duration;animation.repeatCount=loops ? .infinity:0
        return animation
    }
}
