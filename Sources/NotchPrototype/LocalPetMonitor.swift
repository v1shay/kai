import Foundation
import CoreServices

/// One recursive filesystem event stream covers the library and Downloads.
/// No polling timer or file descriptor per pet/pack is needed.
@MainActor final class LocalPetMonitor {
    private var stream:FSEventStreamRef?
    private var watched=[String]()
    private var pending:DispatchWorkItem?
    var changed:(()->Void)?
    func watch(_ roots:[URL]) {
        let paths=Array(Set(roots.filter{FileManager.default.fileExists(atPath:$0.path)}.map{$0.resolvingSymlinksInPath().path})).sorted()
        guard paths != watched else{return};stop();guard !paths.isEmpty else{return}
        var context=FSEventStreamContext(version:0,info:Unmanaged.passUnretained(self).toOpaque(),retain:nil,release:nil,copyDescription:nil)
        let callback:FSEventStreamCallback={_,info,_,_,_,_ in
            guard let info else{return}
            MainActor.assumeIsolated{Unmanaged<LocalPetMonitor>.fromOpaque(info).takeUnretainedValue().schedule()}
        }
        let flags=FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagWatchRoot | kFSEventStreamCreateFlagNoDefer)
        guard let stream=FSEventStreamCreate(nil,callback,&context,paths as CFArray,FSEventStreamEventId(kFSEventStreamEventIdSinceNow),0.2,flags)else{return}
        FSEventStreamSetDispatchQueue(stream,DispatchQueue.main)
        guard FSEventStreamStart(stream)else{FSEventStreamInvalidate(stream);FSEventStreamRelease(stream);return}
        self.stream=stream;watched=paths
    }
    private func schedule(){pending?.cancel();let work=DispatchWorkItem{[weak self] in self?.pending=nil;self?.changed?()};pending=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.5,execute:work)}
    func stop(){pending?.cancel();pending=nil;if let stream{FSEventStreamStop(stream);FSEventStreamInvalidate(stream);FSEventStreamRelease(stream)};stream=nil;watched=[]}
}
