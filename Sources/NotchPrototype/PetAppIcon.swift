import AppKit
import ImageIO
import UniformTypeIdentifiers

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


/// Immutable inputs and private bitmap contexts; no AppKit work leaves the main actor.
struct PetIconStillRequest: @unchecked Sendable {
    let sheet:CGImage,rows:Int,row:Int,column:Int
    func matches(_ other:Self)->Bool{sheet === other.sheet && rows == other.rows && row == other.row && column == other.column}
}
struct PetIconFrameRequest: @unchecked Sendable {
    let sheet:CGImage,rows:Int,row:Int,columns:[Int],still:PetIconStillRequest?
    func matches(_ other:Self)->Bool {
        sheet === other.sheet && rows == other.rows && row == other.row && columns == other.columns && ((still == nil && other.still == nil) || (still != nil && other.still != nil && still!.matches(other.still!)))
    }
}
struct PetIconFrameResult: @unchecked Sendable {
    var frames:[Int:CGImage]=[:]
    var still:CGImage?
}
@MainActor final class PetIconFramePipeline {
    typealias Executor = (PetIconFrameRequest,@escaping(PetIconFrameResult)->Void)->Void
    private let execute:Executor
    private var active:PetIconFrameRequest?,pending:PetIconFrameRequest?,current:PetIconFrameRequest?
    private var deliver:((PetIconFrameResult)->Void)?
    private var revision=0,activeRevision=0
    var activeCount:Int{active == nil ? 0:1}
    var pendingCount:Int{pending == nil ? 0:1}
    init(execute:Executor?=nil){
        let queue=DispatchQueue(label:"kai.dock-frame-raster",qos:.userInitiated)
        self.execute=execute ?? { request,done in
            queue.async {
                assert(!Thread.isMainThread)
                let result=autoreleasepool { Self.render(request) }
                DispatchQueue.main.async {done(result)}
            }
        }
    }
    func submit(_ request:PetIconFrameRequest,completion:@escaping(PetIconFrameResult)->Void){
        if let current,current.matches(request){deliver=completion;return}
        revision &+= 1;current=request;deliver=completion
        if active != nil {pending=request}else{begin(request)}
    }
    func cancel(){revision &+= 1;current=nil;pending=nil;deliver=nil}
    private func begin(_ request:PetIconFrameRequest){
        active=request;activeRevision=revision;let token=activeRevision
        execute(request){[weak self] result in
            MainActor.assumeIsolated {
                guard let self else{return}
                self.active=nil
                if token == self.revision,self.current?.matches(request) == true {
                    let callback=self.deliver;self.current=nil;self.deliver=nil;callback?(result)
                }
                if let next=self.pending {self.pending=nil;self.begin(next)}
            }
        }
    }
    nonisolated static func render(_ request:PetIconFrameRequest)->PetIconFrameResult {
        var result=PetIconFrameResult()
        for column in request.columns {result.frames[column]=frame(sheet:request.sheet,rows:request.rows,row:request.row,column:column,size:128)}
        if let still=request.still {result.still=frame(sheet:still.sheet,rows:still.rows,row:still.row,column:still.column,size:512)}
        return result
    }
    nonisolated static func frame(sheet:CGImage,rows:Int,row:Int,column:Int,size:Int)->CGImage? {
        guard rows>0,(0..<rows).contains(row),(0..<8).contains(column),size>0 else{return nil}
        let width=sheet.width/8,height=sheet.height/rows
        guard width>0,height>0,
              let crop=sheet.cropping(to:CGRect(x:column*width,y:row*height,width:width,height:height)),
              let context=CGContext(data:nil,width:size,height:size,bitsPerComponent:8,bytesPerRow:size*4,
                                    space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else{return nil}
        let scale=CGFloat(size)*0.92/CGFloat(max(width,height)),w=CGFloat(width)*scale,h=CGFloat(height)*scale
        context.interpolationQuality = .high
        context.draw(crop,in:CGRect(x:(CGFloat(size)-w)/2,y:(CGFloat(size)-h)/2,width:w,height:h))
        return context.makeImage()
    }
}

@MainActor final class PetAppIcon {
    var onCompletion:(()->Void)?
    private let view=NSImageView(frame:CGRect(x:0,y:0,width:128,height:128))
    private var petID="",sheet:CGImage?,rows=0,row=0
    private var playback:PetIconPlayback?,started=0.0
    private var frames=[Int:NSImage](),displayedColumn:Int?
    private var timer:Timer?,completionWork:DispatchWorkItem?
    private var observers=[NSObjectProtocol](),pauses=Set<String>()
    private var still:NSImage?
    private let framePipeline:PetIconFramePipeline
    private let iconWriter = DispatchQueue(label: "kai.dock-icon-save", qos: .utility)
    private var frameRequest:PetIconFrameRequest?
    private var requestedStill:PetIconStillRequest?
    private var working=false
    func setWorking(_ value:Bool){guard value != working else{return};working=value;refreshTimer()}

    init(execute:PetIconFramePipeline.Executor?=nil){
        framePipeline=PetIconFramePipeline(execute:execute)
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

    func play(petID:String,sheet:CGImage,rows:Int,row:Int,columns:[Int],durationsMs:[Int],loops:Bool,stillCell:(row:Int,column:Int)?=nil){
        guard rows>0,(0..<rows).contains(row),let playback=PetIconPlayback(columns:columns,durationsMs:durationsMs,loops:loops) else{return}
        let changed=self.petID != petID
        if changed || self.row != row || self.rows != rows || self.sheet !== sheet{frames.removeAll(keepingCapacity:true);frameRequest=nil}
        self.petID=petID;self.sheet=sheet;self.rows=rows;self.row=row
        self.playback=playback;started=CACurrentMediaTime();displayedColumn=nil
        completionWork?.cancel();completionWork=nil
        if !loops {
            let work=DispatchWorkItem{[weak self] in self?.onCompletion?()};completionWork=work
            DispatchQueue.main.asyncAfter(deadline:.now()+playback.duration,execute:work)
        }
        if changed {requestedStill=PetIconStillRequest(sheet:sheet,rows:rows,row:stillCell?.row ?? row,column:stillCell?.column ?? columns[0])}
        else if let pending=requestedStill,pending.sheet !== sheet {
            // A library refresh may keep petID while replacing its immutable sheet.
            // Keep the original still cell across motion changes, but never publish old library pixels.
            requestedStill=PetIconStillRequest(sheet:sheet,rows:rows,row:(0..<rows).contains(pending.row) ? pending.row:row,column:pending.column)
        }
        let request=PetIconFrameRequest(sheet:sheet,rows:rows,row:row,columns:Array(Set(columns)).sorted(),still:requestedStill)
        if frameRequest?.matches(request) != true {
            frameRequest=request
            framePipeline.submit(request){[weak self] result in
                guard let self,self.frameRequest?.matches(request) == true else{return}
                self.frames=result.frames.mapValues{NSImage(cgImage:$0,size:CGSize(width:128,height:128))}
                if let image=result.still {
                    let still=NSImage(cgImage:image,size:CGSize(width:512,height:512));self.still=still
                    NSApp.applicationIconImage=still
                    self.saveDockStill(result)
                    // AppKit can replace Dock contentView when the application icon changes.
                    guard self.frameRequest?.matches(request) == true else{return}
                    NSApp.dockTile.contentView=self.view
                }
                self.requestedStill=nil
                self.frameRequest=PetIconFrameRequest(sheet:request.sheet,rows:request.rows,row:request.row,columns:request.columns,still:nil)
                self.displayedColumn=nil;self.refreshTimer()
            }
        }
        NSApp.dockTile.contentView=view
        refreshTimer()
    }

    private func saveDockStill(_ result: PetIconFrameResult) {
        guard NativePreferencePersistence.enabled else { return }
        iconWriter.async {
            guard let image = result.still,
                  let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return }
            let directory = support.appendingPathComponent("Kai Unified", isDirectory: true)
            let png = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(png, UTType.png.identifier as CFString, 1, nil) else { return }
            CGImageDestinationAddImage(destination, image, nil)
            guard CGImageDestinationFinalize(destination) else { return }
            do {
                try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                try png.write(to: directory.appendingPathComponent("DockPet.png"), options: .atomic)
                DistributedNotificationCenter.default().postNotificationName(
                    Notification.Name("com.kai.unified.dock-pet-changed"), object: nil, userInfo: nil, deliverImmediately: true)
            } catch {
                NSLog("Could not save Kai’s Dock icon: %@", error.localizedDescription)
            }
        }
    }

    private func refreshTimer(){
        timer?.invalidate();timer=nil
        guard let playback else{return}
        let reduceMotion=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        display(column:reduceMotion ? playback.columns[0]:playback.column(at:CACurrentMediaTime()-started))
        guard pauses.isEmpty,!reduceMotion,(working || !playback.loops),Set(playback.columns).count>1,
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
        guard displayedColumn != column else{return}
        guard let image=frames[column] else{return}
        displayedColumn=column;view.image=image;NSApp.dockTile.display()
    }

    func stop(){
        framePipeline.cancel();frameRequest=nil;requestedStill=nil
        timer?.invalidate();timer=nil
        completionWork?.cancel();completionWork=nil
        observers.forEach{NSWorkspace.shared.notificationCenter.removeObserver($0)};observers=[]
        NSApp.dockTile.contentView=nil
        if let still{NSApp.applicationIconImage=still}
    }

    /// Catalog coordinates use a top-left origin, as does CGImage cropping.
    static func frame(sheet:CGImage,rows:Int,row:Int,column:Int,size:Int)->NSImage? {
        guard let image=PetIconFramePipeline.frame(sheet:sheet,rows:rows,row:row,column:column,size:size) else{return nil}
        return NSImage(cgImage:image,size:CGSize(width:size,height:size))
    }
}
