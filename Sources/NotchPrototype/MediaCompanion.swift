import AppKit
import ApplicationServices
import ScreenCaptureKit
import AVFoundation
import os

struct MediaNowPlaying: Equatable {
    let identity: String, title: String, artwork: String, application: String
}

/// Reads media metadata without navigating tabs or changing playback.
@MainActor final class MediaCompanion {
    var onChange: ((MediaNowPlaying?) -> Void)?
    var onLevels: (([CGFloat]) -> Void)?
    var onStatus: ((String) -> Void)?
    var youtube = false, spotify = false
    private var timer: Timer?, busy = false, generation = 0
    private var current: MediaNowPlaying?
    private let meter = PlaybackMeter()
    static func codexNeedsNotch(_ state:KaiState)->Bool {
        state.activeTurnId != nil || !state.activeTasks.isEmpty || state.approval || !["off","failure"].contains(state.activity.scene)
    }
    func configure(youtube:Bool,spotify:Bool) {
        generation += 1; self.youtube=youtube;self.spotify=spotify;timer?.invalidate();timer=nil
        if !youtube && !spotify {current=nil;onChange?(nil);meter.stop();return}
        timer=Timer.scheduledTimer(withTimeInterval:1,repeats:true){[weak self] _ in Task{ @MainActor [weak self] in self?.poll()}}
        if let timer {RunLoop.main.add(timer,forMode:.common)}
        poll()
    }
    func diagnose(){meter.diagnose()}
    func capture(_ enabled:Bool) {
        if enabled, let current {
            meter.onLevels=onLevels;meter.onError=onStatus;meter.start(application:current.application)
        }else{meter.stop()}
    }
    private func poll() {
        guard !busy else{return};busy=true
        let gen=generation, useYouTube=youtube,useSpotify=spotify
        let apps=NSWorkspace.shared.runningApplications.filter { app in
            (useSpotify && app.localizedName == "Spotify") || (useYouTube && ["Webby","Dia","Google Chrome"].contains(app.localizedName ?? ""))
        }.map{($0.processIdentifier,$0.localizedName ?? "",$0.isActive)}.sorted{$0.2 && !$1.2}
        DispatchQueue.global(qos:.utility).async { [weak self] in
            var found:MediaNowPlaying?
            for (pid,name,_) in apps {
                if name == "Spotify" {found=Self.spotifyState()}
                else {found=Self.youtubeState(pid:pid,name:name)}
                if found != nil{break}
            }
            let value=found
            DispatchQueue.main.async { [weak self] in
                guard let self else{return};self.busy=false;guard gen == self.generation else{return}
                self.current=value;self.onChange?(value)
            }
        }
    }
    private nonisolated static func spotifyState()->MediaNowPlaying? {
        let source="""
        tell application "Spotify"
            if player state is not playing then return ""
            return (id of current track) & linefeed & (name of current track) & linefeed & (artwork url of current track)
        end tell
        """
        var error:NSDictionary?
        guard let script=NSAppleScript(source:source),let result=script.executeAndReturnError(&error).stringValue,error == nil else{return nil}
        let rows=result.components(separatedBy:"\n");guard rows.count >= 3 else{return nil}
        return MediaNowPlaying(identity:rows[0],title:rows[1],artwork:rows[2],application:"Spotify")
    }
    private nonisolated static func attribute(_ element:AXUIElement,_ name:String)->AnyObject? {
        var value:CFTypeRef?;guard AXUIElementCopyAttributeValue(element,name as CFString,&value) == .success else{return nil};return value
    }
    private nonisolated static func youtubeState(pid:pid_t,name:String)->MediaNowPlaying? {
        guard AXIsProcessTrusted() else{return nil}
        let app=AXUIElementCreateApplication(pid)
        guard let windows=attribute(app,kAXWindowsAttribute) as? [AXUIElement] else{return nil}
        for window in windows.prefix(8) {
            var queue=[window],cursor=0,url:String?,playing=false,title=attribute(window,kAXTitleAttribute) as? String ?? "YouTube"
            // Bounded AX traversal. The pause control is the playback signal, not a YouTube URL alone.
            while cursor < queue.count && cursor < 1500 {
                let node=queue[cursor];cursor += 1
                for key in [kAXURLAttribute,kAXValueAttribute] {
                    let value=attribute(node,key)
                    let string=(value as? URL)?.absoluteString ?? (value as? String) ?? ""
                    if videoID(string) != nil {url=string}
                }
                let role=attribute(node,kAXRoleAttribute) as? String ?? ""
                let label=[kAXTitleAttribute,kAXDescriptionAttribute,kAXHelpAttribute].compactMap{attribute(node,$0) as? String}.joined(separator:" ").lowercased()
                if role == kAXButtonRole && (label == "pause" || label.hasPrefix("pause ") || label.contains("pause (k)")) {playing=true}
                if let children=attribute(node,kAXChildrenAttribute) as? [AXUIElement] {queue.append(contentsOf:children.prefix(200))}
            }
            if playing,let url,let id=videoID(url) {
                title=title.replacingOccurrences(of:" - YouTube",with:"")
                return MediaNowPlaying(identity:id,title:title,artwork:"https://i.ytimg.com/vi/\(id)/hqdefault.jpg",application:name)
            }
        }
        return nil
    }
    nonisolated static func videoID(_ string:String)->String? {
        let raw=string.hasPrefix("http") ? string:"https://"+string
        guard let url=URLComponents(string:raw),let host=url.host?.lowercased(),["youtube.com","www.youtube.com","m.youtube.com","youtu.be"].contains(host) else{return nil}
        let parts=url.path.split(separator:"/")
        let id:String?
        if host == "youtu.be" {id=parts.first.map(String.init)}
        else if parts.first == "shorts" || parts.first == "embed" || parts.first == "live" {id=parts.count > 1 ? String(parts[1]):nil}
        else {id=url.queryItems?.first{$0.name == "v"}?.value}
        guard let id,id.count == 11,id.allSatisfy({$0.isLetter || $0.isNumber || $0 == "_" || $0 == "-"}) else{return nil};return id
    }
    func stop(){generation += 1;timer?.invalidate();timer=nil;meter.stop()}
}

/// Measures the speaker mix while media is visible. Audio/video buffers are discarded.
final class PlaybackMeter: NSObject, SCStreamOutput, SCStreamDelegate {
    var onLevels: (([CGFloat])->Void)?, onError:((String)->Void)?
    private var stream:SCStream?, generation=0, application=""
    private let logger=Logger(subsystem:"com.kai.notchprototype",category:"media-audio")
    private var silenceTimer:Timer?,lastLevelAt=0.0,reportedAudio=false,retryAfter=Date.distantPast
    private let queue=DispatchQueue(label:"kai.media.audio",qos:.userInteractive)
    private var lastSample=0.0
    private(set) var audioPackets=0, peak:CGFloat=0
    func diagnose(){logger.info("Capture active=\(self.stream != nil) packets=\(self.audioPackets) peak=\(Double(self.peak)) legacyPermission=\(CGPreflightScreenCaptureAccess())")}
    func start(application:String) {
        guard self.application != application,Date() >= retryAfter else{return};stop()
        // The legacy preflight can remain false after access is granted. Let
        // ScreenCaptureKit attempt capture and report its actual result instead.
        self.application=application;reportedAudio=false
        let token=generation
        SCShareableContent.getExcludingDesktopWindows(true,onScreenWindowsOnly:false){[weak self] content,error in
            DispatchQueue.main.async {
                guard let self,token == self.generation else{return}
                guard let content,let display=content.displays.first else{self.application="";self.retryAfter=Date().addingTimeInterval(5);self.report(error?.localizedDescription ?? "No display available for speaker capture");return}
                // WebKit/Chromium render audio in helper processes. Capture the actual
                // output mix rather than trying to guess which helper owns playback.
                let filter=SCContentFilter(display:display,excludingWindows:[])
                let config=SCStreamConfiguration();config.width=2;config.height=2;config.minimumFrameInterval=CMTime(value:1,timescale:1);config.showsCursor=false;config.queueDepth=3;config.capturesAudio=true;config.excludesCurrentProcessAudio=true;config.sampleRate=24000;config.channelCount=1
                let stream=SCStream(filter:filter,configuration:config,delegate:self);self.stream=stream
                do {
                    try stream.addStreamOutput(self,type:.screen,sampleHandlerQueue:self.queue)
                    try stream.addStreamOutput(self,type:.audio,sampleHandlerQueue:self.queue)
                    stream.startCapture{ [weak self, weak stream] error in DispatchQueue.main.async{ [weak self, weak stream] in
                        guard let self,self.stream === stream else{return}
                        if let error{self.report(error.localizedDescription);self.stop();self.retryAfter=Date().addingTimeInterval(5)}
                        else{self.report("Speaker audio connected");self.startSilenceTimer()}
                    }}
                }catch{self.report(error.localizedDescription);self.stop();self.retryAfter=Date().addingTimeInterval(5)}
            }
        }
    }
    private func report(_ status:String){
        logger.info("\(status,privacy:.public)")
        onError?(status.contains("declined TCC") ? "macOS denied capture for this Kai build — open allow media permissions…" : status)
    }
    private func startSilenceTimer(){
        lastLevelAt=ProcessInfo.processInfo.systemUptime
        silenceTimer?.invalidate()
        silenceTimer=Timer.scheduledTimer(withTimeInterval:0.1,repeats:true){[weak self] _ in
            guard let self else{return}
            if ProcessInfo.processInfo.systemUptime-self.lastLevelAt > 0.25 {self.onLevels?([0,0,0,0])}
        }
        if let silenceTimer {RunLoop.main.add(silenceTimer,forMode:.common)}
    }
    func stop(){generation += 1;application="";silenceTimer?.invalidate();silenceTimer=nil;stream?.stopCapture(completionHandler:nil);stream=nil;onLevels?([0,0,0,0])}
    func stream(_ stream:SCStream,didOutputSampleBuffer sampleBuffer:CMSampleBuffer,of outputType:SCStreamOutputType) {
        guard outputType == .audio,sampleBuffer.isValid else{return}
        let now=ProcessInfo.processInfo.systemUptime;guard now-lastSample > 0.035 else{return};lastSample=now
        var needed=0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sampleBuffer,bufferListSizeNeededOut:&needed,bufferListOut:nil,bufferListSize:0,blockBufferAllocator:nil,blockBufferMemoryAllocator:nil,flags:0,blockBufferOut:nil)
        guard needed > 0 else{return}
        let raw=UnsafeMutableRawPointer.allocate(byteCount:needed,alignment:MemoryLayout<AudioBufferList>.alignment);defer{raw.deallocate()}
        let list=raw.bindMemory(to:AudioBufferList.self,capacity:1);var block:CMBlockBuffer?
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sampleBuffer,bufferListSizeNeededOut:nil,bufferListOut:list,bufferListSize:needed,blockBufferAllocator:nil,blockBufferMemoryAllocator:nil,flags:0,blockBufferOut:&block) == noErr else{return}
        var bands=[CGFloat](repeating:0,count:4)
        for buffer in UnsafeMutableAudioBufferListPointer(list) {
            guard let data=buffer.mData else{continue};let count=Int(buffer.mDataByteSize)/MemoryLayout<Float>.size;guard count > 0 else{continue}
            let values=data.assumingMemoryBound(to:Float.self)
            let measured=Self.rmsBands(UnsafeBufferPointer(start:values,count:count))
            for band in 0..<4 {bands[band]=max(bands[band],measured[band])}
        }
        DispatchQueue.main.async{[weak self,weak stream] in guard let self,self.stream === stream else{return};self.audioPackets += 1;self.peak=max(self.peak,bands.max() ?? 0);self.lastLevelAt=ProcessInfo.processInfo.systemUptime;if !self.reportedAudio{self.reportedAudio=true;self.report("Speaker audio reactive")} ;self.onLevels?(bands)}
    }
    static func rmsBands(_ samples:UnsafeBufferPointer<Float>)->[CGFloat] {
        var result=[CGFloat](repeating:0,count:4)
        for band in 0..<4 {
            var sum:Float=0;let start=samples.count*band/4,end=samples.count*(band+1)/4
            for i in start..<end {let value=samples[i];if value.isFinite{sum += value*value}}
            result[band]=min(1,CGFloat(sqrt(sum/Float(max(1,end-start))))*5)
        }
        return result
    }
    func stream(_ stream:SCStream,didStopWithError error:Error){DispatchQueue.main.async{[weak self] in guard let self,self.stream === stream else{return};self.report(error.localizedDescription);self.stop();self.retryAfter=Date().addingTimeInterval(5)}}
}

extension MediaNowPlaying {
    static func profile(_ image:NSImage)->PetGradientProfile? {
        guard let bitmap=NSBitmapImageRep(data:image.tiffRepresentation ?? Data()) else{return nil}
        var colors=[String]()
        for x in [0.2,0.5,0.8] {
            guard let color=bitmap.colorAt(x:Int(Double(bitmap.pixelsWide-1)*x),y:bitmap.pixelsHigh/2)?.usingColorSpace(.deviceRGB) else{continue}
            colors.append(String(format:"#%02x%02x%02x",Int(color.redComponent*255),Int(color.greenComponent*255),Int(color.blueComponent*255)))
        }
        guard colors.count == 3 else{return nil}
        let recipe=GradientRecipe(angleDegrees:25,cycleDurationMs:2200,stops:colors.enumerated().map{GradientStop(location:Double($0.offset)/2,color:$0.element)})
        return PetGradientProfile(palette:PetPalette(shadow:colors[0],primary:colors[0],secondary:colors[1],accent:colors[2],highlight:"#ffffff",foreground:"#ffffff"),gradients:PetGradients(ambient:recipe,thinking:recipe,working:recipe,success:recipe,warning:recipe,error:recipe))
    }
}
