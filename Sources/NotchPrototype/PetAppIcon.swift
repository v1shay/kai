import AppKit

/// Uses the sprite's original timing while limiting Dock IPC to ten updates/second.
struct PetIconPlayback {
    let columns:[Int]
    let durations:[Double]
    let loops:Bool
    var duration:Double{durations.reduce(0,+)}

    init?(columns:[Int],durationsMs:[Int],loops:Bool){
        guard !columns.isEmpty,columns.count == durationsMs.count,
              columns.allSatisfy({(0..<8).contains($0)}) else{return nil}
        self.columns=columns;durations=durationsMs.map{Double(max(16,$0))/1000};self.loops=loops
    }

    func column(at elapsed:Double)->Int {
        if !loops,elapsed>=duration{return columns.last!}
        var remaining=max(0,elapsed).truncatingRemainder(dividingBy:duration)
        for (index,duration) in durations.enumerated(){
            if remaining<duration{return columns[index]};remaining -= duration
        }
        return columns.last!
    }
}

@MainActor final class PetAppIcon {
    var onCompletion:(()->Void)?
    private let view=NSImageView(frame:CGRect(x:0,y:0,width:128,height:128))
    private var petID="",sheet:CGImage?,rows=0,row=0
    private var playback:PetIconPlayback?,started=0.0
    private var frames=[Int:NSImage](),displayedColumn:Int?
    private var timer:Timer?,persistWork:DispatchWorkItem?,completionWork:DispatchWorkItem?
    private var observers=[NSObjectProtocol](),pauses=Set<String>()
    private var still:NSImage?

    init(){
        view.imageScaling = .scaleProportionallyUpOrDown
        let center=NSWorkspace.shared.notificationCenter
        let notifications:[(String,Notification.Name,Notification.Name)]=[
            ("system",NSWorkspace.willSleepNotification,NSWorkspace.didWakeNotification),
            ("screen",NSWorkspace.screensDidSleepNotification,NSWorkspace.screensDidWakeNotification),
            ("session",NSWorkspace.sessionDidResignActiveNotification,NSWorkspace.sessionDidBecomeActiveNotification)
        ]
        for (reason,pause,resume) in notifications {
            observers.append(center.addObserver(forName:pause,object:nil,queue:.main){[weak self] _ in
                MainActor.assumeIsolated{self?.pauses.insert(reason);self?.refreshTimer()}
            })
            observers.append(center.addObserver(forName:resume,object:nil,queue:.main){[weak self] _ in
                MainActor.assumeIsolated{self?.pauses.remove(reason);self?.refreshTimer()}
            })
        }
        observers.append(center.addObserver(forName:NSWorkspace.accessibilityDisplayOptionsDidChangeNotification,object:nil,queue:.main){[weak self] _ in
            MainActor.assumeIsolated{self?.refreshTimer()}
        })
    }

    func play(petID:String,sheet:CGImage,rows:Int,row:Int,columns:[Int],durationsMs:[Int],loops:Bool){
        guard rows>0,(0..<rows).contains(row),let playback=PetIconPlayback(columns:columns,durationsMs:durationsMs,loops:loops) else{return}
        let changed=self.petID != petID
        if changed || self.row != row || self.sheet !== sheet{frames.removeAll(keepingCapacity:true)}
        self.petID=petID;self.sheet=sheet;self.rows=rows;self.row=row
        self.playback=playback;started=CACurrentMediaTime();displayedColumn=nil
        completionWork?.cancel();completionWork=nil
        if !loops {
            let work=DispatchWorkItem{[weak self] in self?.onCompletion?()};completionWork=work
            DispatchQueue.main.asyncAfter(deadline:.now()+playback.duration,execute:work)
        }
        if changed {
            still=Self.frame(sheet:sheet,rows:rows,row:row,column:columns[0],size:512)
            if let still{NSApp.applicationIconImage=still}
            persistWork?.cancel()
            let work=DispatchWorkItem{[weak self] in self?.persistStill()};persistWork=work
            DispatchQueue.main.asyncAfter(deadline:.now()+0.5,execute:work)
        }
        NSApp.dockTile.contentView=view
        refreshTimer()
    }

    private func refreshTimer(){
        timer?.invalidate();timer=nil
        guard let playback else{return}
        let reduceMotion=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        display(column:reduceMotion ? playback.columns[0]:playback.column(at:CACurrentMediaTime()-started))
        guard pauses.isEmpty,!reduceMotion,Set(playback.columns).count>1,
              playback.loops || CACurrentMediaTime()-started<playback.duration else{return}
        let timer=Timer(timeInterval:0.1,repeats:true){[weak self] _ in
            MainActor.assumeIsolated{self?.tick()}
        }
        timer.tolerance=0.02;RunLoop.main.add(timer,forMode:.common);self.timer=timer
    }

    private func tick(){
        guard let playback else{return}
        let elapsed=CACurrentMediaTime()-started
        display(column:playback.column(at:elapsed))
        if !playback.loops,elapsed>=playback.duration{timer?.invalidate();timer=nil}
    }

    private func display(column:Int){
        guard displayedColumn != column,let sheet else{return}
        if frames[column] == nil{frames[column]=Self.frame(sheet:sheet,rows:rows,row:row,column:column,size:128)}
        guard let image=frames[column] else{return}
        displayedColumn=column;view.image=image;NSApp.dockTile.display()
    }

    /// Only selection changes touch the filesystem; animation frames never do.
    private func persistStill(){
        persistWork=nil
        guard let still,Bundle.main.bundleURL.pathExtension == "app" else{return}
        if !NSWorkspace.shared.setIcon(still,forFile:Bundle.main.bundlePath,options:[]){
            NSLog("Kai could not persist the pet icon at %@",Bundle.main.bundlePath)
        }
    }

    func stop(){
        timer?.invalidate();timer=nil
        completionWork?.cancel();completionWork=nil
        if persistWork != nil{persistWork?.cancel();persistStill()}
        observers.forEach{NSWorkspace.shared.notificationCenter.removeObserver($0)};observers=[]
        NSApp.dockTile.contentView=nil
        if let still{NSApp.applicationIconImage=still}
    }

    /// Catalog coordinates use a top-left origin, as does CGImage cropping.
    static func frame(sheet:CGImage,rows:Int,row:Int,column:Int,size:Int)->NSImage? {
        guard rows>0,(0..<rows).contains(row),(0..<8).contains(column),size>0 else{return nil}
        let width=sheet.width/8,height=sheet.height/rows
        guard width>0,height>0,
              let crop=sheet.cropping(to:CGRect(x:column*width,y:row*height,width:width,height:height)),
              let context=CGContext(data:nil,width:size,height:size,bitsPerComponent:8,bytesPerRow:size*4,
                                    space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else{return nil}
        let scale=CGFloat(size)*0.92/CGFloat(max(width,height)),w=CGFloat(width)*scale,h=CGFloat(height)*scale
        context.interpolationQuality = .high
        context.draw(crop,in:CGRect(x:(CGFloat(size)-w)/2,y:(CGFloat(size)-h)/2,width:w,height:h))
        guard let image=context.makeImage() else{return nil}
        return NSImage(cgImage:image,size:CGSize(width:size,height:size))
    }
}
