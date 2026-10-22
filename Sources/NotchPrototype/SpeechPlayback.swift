import AppKit
import AVFoundation

/// A warm Piper process, with bounded streaming phrases and cancellable playback.
@MainActor final class SpeechPlayback {
    var onModels: (([String]) -> Void)?
    var onStatus: ((String) -> Void)?
    private(set) var models = [String]()
    private var process: Process?, input: Pipe?, output: Pipe?
    private var buffer = Data(), token = UUID().uuidString
    private let engine = AVAudioEngine(), player = AVAudioPlayerNode()
    private var format: AVAudioFormat?
    private var seen = [String: String](), thread: String?, pending = ""
    private var flush: DispatchWorkItem?
    private var enabled = false
    private var queuedBuffers=0
    private var lastQueuedPhrase:String?
    private var inFlight = 0, phrases = [String](), warm = false
    private var requestStarted = Date(),wasActive=false
    var selectedModel = ""

    func discover() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let task = Process(), pipe = Pipe()
            task.executableURL = URL(fileURLWithPath: "/usr/bin/mdfind")
            task.arguments = ["kMDItemFSName == '*.onnx'"]; task.standardOutput = pipe
            var paths = [String]()
            if (try? task.run()) != nil {
                let data = pipe.fileHandleForReading.readDataToEndOfFile(); task.waitUntilExit()
                paths = String(decoding:data,as:UTF8.self).split(separator:"\n").map(String.init)
            }
            let home = FileManager.default.homeDirectoryForCurrentUser
            for folder in [home.appendingPathComponent(".local/share/piper"), home.appendingPathComponent(".cache/piper")] {
                if let entries = FileManager.default.enumerator(at:folder,includingPropertiesForKeys:nil) {
                    for case let url as URL in entries where url.pathExtension == "onnx" {paths.append(url.path)}
                }
            }
            let compatible = Array(Set(paths.filter { path in
                guard let data = FileManager.default.contents(atPath:path+".json"),
                      let config = try? JSONSerialization.jsonObject(with:data) as? [String:Any] else{return false}
                return config["phoneme_id_map"] != nil && config["audio"] != nil
            })).sorted()
            DispatchQueue.main.async { [weak self] in self?.models = compatible;self?.onModels?(compatible)}
        }
    }

    func configure(enabled: Bool, model: String) {
        stop(); self.enabled = enabled; selectedModel = model
        guard enabled, !model.isEmpty else{return}
        onStatus?("Loading local voice…")
        let sourceRoot=URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let root=Bundle.main.resourceURL.flatMap{FileManager.default.fileExists(atPath:$0.appendingPathComponent("kai_speech.py").path) ? $0:nil} ?? sourceRoot
        let script = root.appendingPathComponent("kai_speech.py")
        let task = Process(), incoming = Pipe(), outgoing = Pipe()
        // Prefer the configured runtime, then the locally discovered Piper installation.
        let configured = UserDefaults.standard.string(forKey:"speechPython")
        let runtime = [configured, "/opt/anaconda3/bin/python", "/opt/homebrew/bin/python3", "/usr/local/bin/python3"].compactMap{$0}.first{FileManager.default.isExecutableFile(atPath:$0)} ?? "/usr/bin/python3"
        task.executableURL = URL(fileURLWithPath:runtime);task.arguments=[script.path, model]
        task.standardInput=incoming;task.standardOutput=outgoing;task.standardError=FileHandle.nullDevice
        output=outgoing;input=incoming;process=task
        outgoing.fileHandleForReading.readabilityHandler = { [weak self, weak task] handle in
            let data=handle.availableData
            if data.isEmpty{handle.readabilityHandler=nil;return}
            DispatchQueue.main.async { [weak self, weak task] in guard let self, self.process === task else{return};self.accept(data)}
        }
        task.terminationHandler = { [weak self, weak task] _ in DispatchQueue.main.async { [weak self, weak task] in
            guard let self,self.process === task else{return};self.warm=false;self.onStatus?("Voice unavailable — check Piper runtime")
        }}
        do {try task.run()} catch {onStatus?(error.localizedDescription)}
    }

    func synchronize(_ state:KaiState){
        cancelAudio();thread=state.threadId;wasActive=state.activeTurnId != nil
        seen=Dictionary(state.history.map{($0.id,text($0))},uniquingKeysWith:{_,last in last})
    }
    func receive(_ state: KaiState) {
        guard enabled else{return}
        if thread != state.threadId {wasActive=state.activeTurnId != nil;cancelAudio();seen.removeAll();thread=state.threadId;for message in state.history {seen[message.id]=text(message)};return}
        let canSpeak=state.activeTurnId != nil || wasActive
        wasActive=state.activeTurnId != nil
        var changed=false
        let firstKnown=state.history.firstIndex{seen[$0.id] != nil} ?? 0
        for (index,message) in state.history.enumerated() where message.kind == "assistant" {
            let value=text(message), prior=seen[message.id] ?? ""
            seen[message.id]=value
            guard value.hasPrefix(prior), value.count > prior.count else{continue}
            // Only speak streaming output or the final update of the active turn.
            guard canSpeak,index >= firstKnown else{continue}
            changed=true
            pending += String(value.dropFirst(prior.count))
        }
        drain(final:state.activeTurnId == nil)
        if changed || state.activeTurnId == nil {flush?.cancel()}
        if changed && !pending.isEmpty {
            let work=DispatchWorkItem { [weak self] in self?.drain(final:false) };flush=work
            DispatchQueue.main.asyncAfter(deadline:.now()+0.3,execute:work)
        }
    }
    private func text(_ message:KaiMessage)->String {message.speechText ?? message.lines.map(\.text).joined(separator:"\n")}
    nonisolated static func splitPhrases(_ text:String,final:Bool)->(phrases:[String],remainder:String) {
        var remaining=text,phrases=[String]()
        while !remaining.isEmpty {
            let limit=remaining.index(remaining.startIndex,offsetBy:min(180,remaining.count))
            let prefix=remaining[..<limit]
            let protected=SpeechText.protectedRanges(remaining)
            func safe(_ index:String.Index)->Bool{let location=remaining[..<index].utf16.count;return !protected.contains{NSLocationInRange(location,$0)}}
            let boundary=prefix.indices.first{safe($0) && ".!?\n".contains(remaining[$0])} ?? (remaining.count >= 100 ? prefix.indices.last{safe($0) && remaining[$0] == " "}:nil)
            var fallback=limit
            if final,limit<remaining.endIndex {
                let location=remaining[..<limit].utf16.count
                if let range=protected.first(where:{NSLocationInRange(location,$0)}) {
                    fallback=String.Index(utf16Offset:NSMaxRange(range),in:remaining)
                }
            }
            guard let end=boundary.map({remaining.index(after:$0)}) ?? (final ? fallback:nil) else{break}
            let phrase=SpeechText.readable(String(remaining[..<end]))
            remaining.removeSubrange(..<end);if phrase.rangeOfCharacter(from:.alphanumerics) != nil{phrases.append(phrase)}
        }
        return (phrases,remaining)
    }
    private func drain(final:Bool) {
        let split=Self.splitPhrases(pending,final:final);pending=split.remainder;for phrase in split.phrases where phrase != lastQueuedPhrase{phrases.append(phrase);lastQueuedPhrase=phrase};pump()
    }
    private func pump() {
        guard warm,inFlight == 0,queuedBuffers < 2,!phrases.isEmpty,let input else{return}
        let phrase=phrases.removeFirst();inFlight=1;requestStarted=Date()
        if let data=try? JSONSerialization.data(withJSONObject:["text":phrase,"token":token]) {try? input.fileHandleForWriting.write(contentsOf:data+Data([10]))}
    }
    private func accept(_ data:Data) {
        buffer.append(data)
        while let end=buffer.firstIndex(of:10) {
            let line=buffer[..<end];buffer.removeSubrange(...end)
            guard let obj=try? JSONSerialization.jsonObject(with:line) as? [String:Any] else{continue}
            if obj["ready"] as? Bool == true {warm=true;onStatus?("Local voice ready");pump();continue}
            if let error=obj["error"] as? String {inFlight=0;onStatus?(error);continue}
            if obj["token"] as? String == token,obj["done"] as? Bool == true {inFlight=0;pump();continue}
            guard obj["token"] as? String == token,let rate=obj["rate"] as? Double,let encoded=obj["audio"] as? String,let pcm=Data(base64Encoded:encoded) else{continue}
            play(pcm,rate:rate)
            onStatus?("Voice first audio: \(Int(Date().timeIntervalSince(requestStarted)*1000)) ms")
        }
    }
    private func play(_ pcm:Data,rate:Double) {
        guard rate.isFinite,rate > 0,rate <= 192000,!pcm.isEmpty,pcm.count <= 16*1024*1024,let next=AVAudioFormat(standardFormatWithSampleRate:rate,channels:1) else{onStatus?("Voice returned invalid audio");return}
        if format?.sampleRate != rate {
            engine.stop();if player.engine != nil {engine.detach(player)};engine.attach(player);engine.connect(player,to:engine.mainMixerNode,format:next);format=next
        }
        guard let audio=AVAudioPCMBuffer(pcmFormat:next,frameCapacity:AVAudioFrameCount(pcm.count/2)),let channel=audio.floatChannelData?[0] else{return}
        audio.frameLength=audio.frameCapacity
        pcm.withUnsafeBytes { raw in for i in 0..<Int(audio.frameLength) {channel[i]=Float(Int16(littleEndian:raw.loadUnaligned(fromByteOffset:i*2,as:Int16.self)))/32768} }
        do {if !engine.isRunning{try engine.start()};if !player.isPlaying{player.play()}}catch{onStatus?(error.localizedDescription);inFlight=0;return}
        // Generate the next phrase as soon as this one is queued, while playback runs.
        let playbackToken=token;queuedBuffers += 1
        player.scheduleBuffer(audio,completionCallbackType:.dataPlayedBack){ [weak self] _ in
            DispatchQueue.main.async{ [weak self] in guard let self,self.token == playbackToken else{return};self.queuedBuffers=max(0,self.queuedBuffers-1);self.pump()}
        }
    }
    func cancelAudio() {token=UUID().uuidString;player.stop();queuedBuffers=0;pending="";lastQueuedPhrase=nil;phrases.removeAll();inFlight=0;flush?.cancel()}
    func stop() {cancelAudio();engine.stop();warm=false;output?.fileHandleForReading.readabilityHandler=nil;process?.terminationHandler=nil;if process?.isRunning == true{process?.terminate()};process=nil;input=nil;output=nil;buffer.removeAll();seen.removeAll();thread=nil}
}
