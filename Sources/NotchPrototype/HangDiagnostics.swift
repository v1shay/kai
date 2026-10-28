import AppKit

struct HangSampleGate {
    private(set) var lastSample = -Double.infinity
    private(set) var samples=0
    mutating func shouldSample(now:Double,heartbeat:Double)->Bool{
        guard now-heartbeat>5,now-lastSample>120,samples<3 else{return false}
        lastSample=now;samples+=1;return true
    }
}

/// Bounded, content-free hang evidence. Normal operation writes no periodic
/// logs and uses one heartbeat per second; only a stalled main loop is sampled.
final class HangDiagnostics:@unchecked Sendable {
    private let lock=NSLock(),queue=DispatchQueue(label:"kai.hang-diagnostics",qos:.utility)
    private var heartbeat=ProcessInfo.processInfo.systemUptime,event="launch",gate=HangSampleGate(),suspended=false
    private var mainTimer:Timer?,watcher:DispatchSourceTimer?,observers=[NSObjectProtocol]()
    func start(){
        guard mainTimer == nil,!CommandLine.arguments.contains("--unified-smoke-test") else{return}
        let timer=Timer(timeInterval:1,repeats:true){[weak self] _ in self?.beat()};timer.tolerance=0.2;RunLoop.main.add(timer,forMode:.common);mainTimer=timer
        let watcher=DispatchSource.makeTimerSource(queue:queue);watcher.schedule(deadline:.now()+6,repeating:2,leeway:.milliseconds(500));watcher.setEventHandler{[weak self] in self?.check()};self.watcher=watcher;watcher.resume()
        let center=NSWorkspace.shared.notificationCenter
        for (name,value) in [(NSWorkspace.willSleepNotification,true),(NSWorkspace.didWakeNotification,false)]{
            observers.append(center.addObserver(forName:name,object:nil,queue:.main){[weak self] _ in guard let self else{return};self.lock.lock();self.suspended=value;self.heartbeat=ProcessInfo.processInfo.systemUptime;self.lock.unlock()})
        }
    }
    func record(_ value:String){lock.lock();event=value;lock.unlock()}
    private func beat(){lock.lock();heartbeat=ProcessInfo.processInfo.systemUptime;lock.unlock()}
    private func check(){
        lock.lock();let now=ProcessInfo.processInfo.systemUptime,stalled=now-heartbeat,lastEvent=event
        let capture = !suspended && gate.shouldSample(now:now,heartbeat:heartbeat);lock.unlock()
        guard capture else{return}
        let folder=FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Kai Unified/Diagnostics")
        do{
            try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
            let old=((try? FileManager.default.contentsOfDirectory(at:folder,includingPropertiesForKeys:[.creationDateKey],options:.skipsHiddenFiles)) ?? []).filter{$0.lastPathComponent.hasPrefix("hang-") && ["json","txt"].contains($0.pathExtension)}.sorted{((try? $0.resourceValues(forKeys:[.creationDateKey]).creationDate) ?? .distantPast)<((try? $1.resourceValues(forKeys:[.creationDateKey]).creationDate) ?? .distantPast)}
            for file in old.prefix(max(0,old.count-22)){try? FileManager.default.removeItem(at:file)}
            let stem="hang-\(ProcessInfo.processInfo.processIdentifier)-\(Int(now))"
            let metadata:[String:Any]=["mainThreadStallSeconds":stalled,"lastSurfaceEvent":lastEvent,"date":ISO8601DateFormatter().string(from:Date()),"version":Bundle.main.object(forInfoDictionaryKey:"CFBundleShortVersionString") as? String ?? "development"]
            try JSONSerialization.data(withJSONObject:metadata,options:.prettyPrinted).write(to:folder.appendingPathComponent(stem+".json"),options:.atomic)
            let sample=Process();sample.executableURL=URL(fileURLWithPath:"/usr/bin/sample");sample.arguments=[String(ProcessInfo.processInfo.processIdentifier),"3","-file",folder.appendingPathComponent(stem+".sample.txt").path];sample.standardOutput=FileHandle.nullDevice;sample.standardError=FileHandle.nullDevice
            try sample.run();sample.waitUntilExit()
        }catch{NSLog("Kai hang diagnostics: %@",error.localizedDescription)}
    }
    func stop(){mainTimer?.invalidate();mainTimer=nil;watcher?.cancel();watcher=nil;observers.forEach{NSWorkspace.shared.notificationCenter.removeObserver($0)};observers=[]}
}
