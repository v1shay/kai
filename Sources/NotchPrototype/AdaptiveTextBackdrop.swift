import AppKit
import ScreenCaptureKit
import CoreMedia
import CoreVideo

/// Experimental: tiny SDR backdrop grids. Captured buffers never leave this
/// process, enter chat context, touch disk, or feed another model/service.
private final class AdaptiveCaptureFeed:NSObject,SCStreamOutput,SCStreamDelegate {
    let id=UUID(),displayID:CGDirectDisplayID
    var stream:SCStream?
    var onFrame:((UUID,CGDirectDisplayID,Int,Int,[UInt8])->Void)?
    var onError:((UUID,CGDirectDisplayID)->Void)?
    private let queue=DispatchQueue(label:"kai.adaptive-backdrop",qos:.utility)
    init(display:SCDisplay,application:SCRunningApplication?) throws {
        displayID=display.displayID;super.init()
        let filter=SCContentFilter(display:display,excludingApplications:application.map{[$0]} ?? [],exceptingWindows:[])
        let configuration=SCStreamConfiguration()
        configuration.width=96;configuration.height=max(24,min(192,Int((96*Double(display.height)/Double(max(1,display.width))).rounded())))
        configuration.pixelFormat=kCVPixelFormatType_32BGRA;configuration.colorSpaceName=CGColorSpace.sRGB
        configuration.minimumFrameInterval=CMTime(value:1,timescale:2)
        configuration.queueDepth=3;configuration.showsCursor=false;configuration.capturesAudio=false
        let stream=SCStream(filter:filter,configuration:configuration,delegate:self);self.stream=stream
        try stream.addStreamOutput(self,type:.screen,sampleHandlerQueue:queue)
    }
    func start(_ completion:@escaping(Error?)->Void){stream?.startCapture(completionHandler:completion)}
    func stop(){stream?.stopCapture(completionHandler:{_ in});stream=nil}
    func stream(_ stream:SCStream,didStopWithError error:Error){onError?(id,displayID)}
    func stream(_ stream:SCStream,didOutputSampleBuffer sampleBuffer:CMSampleBuffer,of outputType:SCStreamOutputType){
        guard outputType == .screen,CMSampleBufferIsValid(sampleBuffer),let image=CMSampleBufferGetImageBuffer(sampleBuffer),
              let attachments=CMSampleBufferGetSampleAttachmentsArray(sampleBuffer,createIfNecessary:false) as? [[SCStreamFrameInfo:Any]],
              let status=attachments.first?[.status] as? Int,status==SCFrameStatus.complete.rawValue else{return}
        CVPixelBufferLockBaseAddress(image,.readOnly);defer{CVPixelBufferUnlockBaseAddress(image,.readOnly)}
        let width=CVPixelBufferGetWidth(image),height=CVPixelBufferGetHeight(image),stride=CVPixelBufferGetBytesPerRow(image)
        guard width>0,height>0,width<=192,height<=384,let base=CVPixelBufferGetBaseAddress(image) else{return}
        var bytes=[UInt8](repeating:0,count:width*height*4)
        bytes.withUnsafeMutableBytes{target in guard let start=target.baseAddress else{return};for y in 0..<height{memcpy(start.advanced(by:y*width*4),base.advanced(by:y*stride),width*4)}}
        onFrame?(id,displayID,width,height,bytes)
    }
}

@MainActor final class AdaptiveTextBackdrop {
    static let shared=AdaptiveTextBackdrop()
    @MainActor private final class Region {
        let id:String,role:String
        weak var window:NSWindow?
        weak var nativeHost:NSView?
        var area:CGRect,generation:UInt64,changed=Date(),active=true
        var model:((CGPoint,[Double])->[Double])?
        init(id:String,role:String,window:NSWindow,area:CGRect,generation:UInt64){self.id=id;self.role=role;self.window=window;self.area=area;self.generation=generation}
        func global(_ u:Double,_ v:Double)->CGPoint?{
            guard let window,let view=nativeHost ?? window.contentView else{return nil}
            let point:CGPoint
            if nativeHost != nil {point=CGPoint(x:area.minX+u*area.width,y:area.maxY-v*area.height)}
            else {point=CGPoint(x:view.bounds.minX+area.minX+u*area.width,y:view.isFlipped ? view.bounds.minY+area.minY+v*area.height:view.bounds.maxY-area.minY-v*area.height)}
            return window.convertPoint(toScreen:view.convert(point,to:nil))
        }
        var visible:Bool{guard active,let window else{return false};return window.isVisible && !window.isMiniaturized && window.isOnActiveSpace && window.occlusionState.contains(.visible)}
    }
    private struct Frame {let width:Int,height:Int,bytes:[UInt8]}
    private var regions=[String:Region](),feeds=[CGDirectDisplayID:AdaptiveCaptureFeed](),frames=[CGDirectDisplayID:Frame]()
    private var fingerprints=[String:UInt64](),frozen=[Int:Bool](),observers=[NSObjectProtocol](),followups=[DispatchWorkItem]()
    private var enabled=false,suspended=false,authorised=false,syncPending=false,discovering=false,transitioning=false
    private var generation=0,settle=0.5,lastStatus="",lastNativeChange=Date.distantPast,lastPublish=Date.distantPast
    private var publishTask:DispatchWorkItem?
    private var nativeInk:AdaptiveTextInk?,nativeDecision:[String:Any]?

    func receive(_ packet:[String:Any]){
        switch packet["type"] as? String {
        case "adaptiveTextConfigure":configure(enabled:packet["enabled"] as? Bool ?? false,settle:packet["settleMs"] as? Double ?? 500)
        case "adaptiveTextPermission":
            guard enabled else{return}
            _=CGRequestScreenCaptureAccess();authorised=true;scheduleSync()
        case "adaptiveTextRegion":register(packet)
        case "adaptiveTextVisible":
            if let number=packet["window"] as? Int,let ids=packet["ids"] as? [String]{let visible=Set(ids);for region in regions.values where region.nativeHost == nil && region.window?.windowNumber==number{region.active=visible.contains(region.id)};scheduleSync();publish(force:true);scheduleFollowups()}
        case "adaptiveTextFreeze":
            if let window=packet["window"] as? Int,let value=packet["frozen"] as? Bool,frozen[window] != value{frozen[window]=value;publish(force:true);scheduleFollowups()}
        case "adaptiveTextInvalidate":publish(force:true);scheduleFollowups()
        case "adaptiveTextNotch":if enabled,let decision=packet["decision"] as? [String:Any]{nativeDecision=decision;applyNativeDecision()}
        default:break
        }
    }
    private func configure(enabled:Bool,settle:Double){
        self.settle=max(0.2,min(1.5,settle/1000))
        guard self.enabled != enabled else{if enabled{scheduleSync();publish(force:true);scheduleFollowups()};return}
        self.enabled=enabled;generation+=1
        if !enabled{stop();removeObservers();nativeInk?.restore();nativeDecision=nil;fingerprints.removeAll();regions=regions.filter{$0.value.nativeHost != nil};report("off");return}
        installObservers();authorised=CGPreflightScreenCaptureAccess()
        if authorised{scheduleSync()}else{report("permission_required")}
    }
    private func installObservers(){
        guard observers.isEmpty else{return}
        for name in [NSWindow.willCloseNotification,NSWindow.didMoveNotification,NSWindow.didResizeNotification,NSWindow.didBecomeKeyNotification,NSWindow.didChangeOcclusionStateNotification,NSWindow.didDeminiaturizeNotification,NSWindow.didMiniaturizeNotification,NSWindow.didChangeScreenNotification,NSApplication.didChangeScreenParametersNotification]{
            observers.append(NotificationCenter.default.addObserver(forName:name,object:nil,queue:.main){[weak self] note in MainActor.assumeIsolated{guard let self,self.enabled else{return};if let window=note.object as? NSWindow{for region in self.regions.values where region.window === window{region.changed=Date()}};self.scheduleSync();self.publish(force:true);self.scheduleFollowups()}})
        }
        let workspace=NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.screensDidSleepNotification,NSWorkspace.sessionDidResignActiveNotification]{observers.append(workspace.addObserver(forName:name,object:nil,queue:.main){[weak self]_ in MainActor.assumeIsolated{self?.suspended=true;self?.stop();self?.report("suspended")}})}
        for name in [NSWorkspace.screensDidWakeNotification,NSWorkspace.sessionDidBecomeActiveNotification,NSWorkspace.activeSpaceDidChangeNotification]{observers.append(workspace.addObserver(forName:name,object:nil,queue:.main){[weak self]_ in MainActor.assumeIsolated{guard let self,self.enabled else{return};self.suspended=false;self.scheduleSync();self.scheduleFollowups()}})}
        observers.append(NotificationCenter.default.addObserver(forName:NSApplication.willTerminateNotification,object:nil,queue:.main){[weak self]_ in MainActor.assumeIsolated{self?.stop();self?.removeObservers()}})
        observers.append(NotificationCenter.default.addObserver(forName:NSTextStorage.didProcessEditingNotification,object:nil,queue:.main){[weak self]_ in MainActor.assumeIsolated{self?.refreshNativeInk()}})
    }
    private func removeObservers(){for observer in observers{NotificationCenter.default.removeObserver(observer);NSWorkspace.shared.notificationCenter.removeObserver(observer)};observers.removeAll()}
    private func stop(){publishTask?.cancel();publishTask=nil;lastPublish = .distantPast;generation+=1;discovering=false;for feed in feeds.values{feed.stop()};feeds.removeAll();frames.removeAll();for task in followups{task.cancel()};followups.removeAll()}
    private func register(_ packet:[String:Any]){
        guard enabled,let id=packet["id"] as? String,id.count<=160,let role=packet["role"] as? String,["sidebar","projects","chat","mini","files","agents","changes"].contains(role),let number=packet["window"] as? Int,let rect=packet["rect"] as? [Double],rect.count==4,rect.allSatisfy({$0.isFinite}),rect[2]>0,rect[3]>0,let window=NSApp.windows.first(where:{$0.windowNumber==number}) else{return}
        let area=CGRect(x:rect[0],y:rect[1],width:rect[2],height:rect[3]),revision=(packet["generation"] as? NSNumber)?.uint64Value ?? 0
        if let region=regions[id]{guard revision>=region.generation else{return};region.area=area;region.generation=revision;region.changed=Date()}
        else{if regions.count>=32{regions=regions.filter{$0.value.window != nil}};guard regions.count<32 else{return};regions[id]=Region(id:id,role:role,window:window,area:area,generation:revision)}
        scheduleSync();publish(force:true);scheduleFollowups()
    }
    private func scheduleSync(){guard enabled,!suspended,!syncPending else{return};syncPending=true;DispatchQueue.main.async{[weak self] in guard let self else{return};self.syncPending=false;self.syncStreams()}}
    private func syncStreams(){
        guard enabled,!suspended,authorised else{return}
        regions=regions.filter{$0.value.window != nil}
        fingerprints=fingerprints.filter{regions[$0.key] != nil}
        let windows=Set(regions.values.compactMap{$0.window?.windowNumber})
        frozen=frozen.filter{windows.contains($0.key)}
        let screens=NSScreen.screens
        var wanted=Set<CGDirectDisplayID>()
        for region in regions.values where region.visible{
            guard let window=region.window else{continue}
            for screen in screens where screen.frame.intersects(window.frame){if let id=(screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value{wanted.insert(id)}}
        }
        for id in Array(feeds.keys) where !wanted.contains(id){let feed=feeds.removeValue(forKey:id);frames.removeValue(forKey:id);feed?.stop()}
        let missing=wanted.subtracting(feeds.keys)
        guard !missing.isEmpty else{if wanted.isEmpty{report("paused")};return}
        guard !discovering else{return};discovering=true;let token=generation
        SCShareableContent.getExcludingDesktopWindows(false,onScreenWindowsOnly:true){[weak self] content,error in
            DispatchQueue.main.async{guard let self,self.enabled,!self.suspended,token==self.generation else{return};self.discovering=false
                guard let content else{self.report("capture_error");return}
                guard let own=content.applications.first(where:{$0.processID==ProcessInfo.processInfo.processIdentifier}) else{self.report("capture_error");return}
                var started=false
                for display in content.displays where missing.contains(display.displayID){
                    do{let feed=try AdaptiveCaptureFeed(display:display,application:own);self.feeds[display.displayID]=feed;started=true
                        feed.onFrame={[weak self] id,display,w,h,bytes in DispatchQueue.main.async{guard let self,self.enabled,self.feeds[display]?.id==id else{return};self.frames[display]=Frame(width:w,height:h,bytes:bytes);if self.publish(force:false){self.scheduleFollowups()}}}
                        feed.onError={[weak self] id,display in DispatchQueue.main.async{guard let self,self.feeds[display]?.id==id else{return};self.feeds.removeValue(forKey:display);self.frames.removeValue(forKey:display);self.report("capture_error")}}
                        feed.start{[weak self] error in DispatchQueue.main.async{guard let self,self.feeds[display.displayID]?.id==feed.id else{return};if error != nil{self.feeds.removeValue(forKey:display.displayID);feed.stop();self.report("capture_error")}else{self.report("active");self.applyNativeDecision()}}}
                    }catch{self.report("capture_error")}
                }
                if started{self.scheduleSync()}
            }
        }
    }
    private func report(_ status:String){guard status != lastStatus else{return};lastStatus=status;EmbeddedCodeRuntime.shared.sendAction("adaptive_text_samples",["status":status,"profiles":[]])}
    private func sample(_ global:CGPoint)->[Double]?{
        for screen in NSScreen.screens where screen.frame.contains(global){
            guard let id=(screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value,let frame=frames[id] else{return nil}
            return AdaptiveColorMath.sampleBGRA(frame.bytes,width:frame.width,height:frame.height,screen:screen.frame,point:global)
        }
        return nil
    }
    @discardableResult private func publish(force:Bool)->Bool{
        guard enabled,authorised,!suspended,!frames.isEmpty else{return false}
        let elapsed=Date().timeIntervalSince(lastPublish)
        if elapsed<0.45{
            if publishTask == nil{let token=generation;let task=DispatchWorkItem{[weak self] in guard let self else{return};self.publishTask=nil;guard self.enabled,token==self.generation else{return};self.publish(force:true)};publishTask=task;DispatchQueue.main.asyncAfter(deadline:.now()+0.46-elapsed,execute:task)}
            return false
        }
        lastPublish=Date()
        var profiles=[[String:Any]]()
        for region in regions.values where region.visible{
            var pixels=[[Double]]();var hash:UInt64=1469598103934665603
            let moving=transitioning || (region.window.map{frozen[$0.windowNumber] ?? false} ?? false) || Date().timeIntervalSince(region.changed)<0.18
            for y in 0..<8{for x in 0..<8{
                guard let point=region.global((Double(x)+0.5)/8,(Double(y)+0.5)/8),var rgb=sample(point) else{pixels.append((x+y)%2==0 ? [0,0,0]:[1,1,1]);continue}
                if let host=region.nativeHost,let window=region.window{let local=host.convert(window.convertPoint(fromScreen:point),from:nil);rgb=region.model?(local,rgb) ?? rgb}
                pixels.append(rgb)
            }}
            for value in pixels.joined(){hash=(hash ^ UInt64((value*10000).rounded())) &* 1099511628211};hash=(hash ^ region.generation) &* 1099511628211;hash=(hash ^ (moving ? 1:0)) &* 1099511628211
            if !force && fingerprints[region.id]==hash{continue};fingerprints[region.id]=hash
            profiles.append(["id":region.id,"role":region.role,"generation":region.generation,"pixels":pixels,"rect":[region.area.minX,region.area.minY,region.area.width,region.area.height],"moving":moving])
        }
        if !profiles.isEmpty{EmbeddedCodeRuntime.shared.sendAction("adaptive_text_samples",["status":"active","profiles":profiles])};return !profiles.isEmpty
    }
    private func scheduleFollowups(){
        guard enabled,authorised,!suspended else{return};for task in followups{task.cancel()};followups.removeAll()
        let token=generation
        for delay in [settle+0.2,settle*2+0.5,2.3,2.5+settle,3.4+settle]{let task=DispatchWorkItem{[weak self] in guard let self,self.enabled,token==self.generation else{return};self.publish(force:true)};followups.append(task);DispatchQueue.main.asyncAfter(deadline:.now()+delay,execute:task)}
    }
    func nativeBackgroundChanged(){guard enabled,Date().timeIntervalSince(lastNativeChange)>0.45 else{return};lastNativeChange=Date();publish(force:true);scheduleFollowups()}
    func registerNotch(window:NSWindow,host:NSView,area:CGRect,active:Bool,frozen:Bool,model:@escaping(CGPoint,[Double])->[Double]){
        let id="native-notch",region=regions[id] ?? Region(id:id,role:"notch",window:window,area:area,generation:1)
        if region.area != area || region.active != active{region.generation+=1;region.changed=Date()}
        region.window=window;region.nativeHost=host;region.area=area;region.active=active;region.model=model;regions[id]=region
        if transitioning != frozen{transitioning=frozen;publish(force:true);scheduleFollowups()}
        if !active{nativeInk?.restore()}
        if nativeInk?.host !== host{nativeInk?.restore();nativeInk=AdaptiveTextInk(host:host)}
        if enabled{scheduleSync();applyNativeDecision()}
    }
    private func applyNativeDecision(){
        guard enabled,authorised,regions["native-notch"]?.active == true,let decision=nativeDecision,let color=decision["color"] as? UInt32 else{return}
        nativeInk?.apply(color:color,animated:decision["animated"] as? Bool ?? true)
    }
    func refreshNativeInk(){guard enabled,regions["native-notch"]?.active == true else{return};nativeInk?.refresh()}
}
