import AppKit
import AVFoundation

/// Each subprocess has its own framing state, owned by its decode queue.
private final class SpeechReplyDecoder {
    enum Reply {
        case ready, error(String?, String), done(String), audio(String, AVAudioPCMBuffer)
    }

    private var buffer = Data()

    func receive(_ data: Data) -> [Reply] {
        buffer.append(data)
        var replies: [Reply] = []
        var start = buffer.startIndex
        while let end = buffer[start...].firstIndex(of: 10) {
            if let reply = Self.decode(buffer[start..<end]) { replies.append(reply) }
            start = buffer.index(after: end)
        }
        buffer.removeSubrange(..<start)
        return replies
    }

    private static func decode(_ line: Data) -> Reply? {
        guard let object = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any] else { return nil }
        if object["ready"] as? Bool == true { return .ready }
        if let error = object["error"] as? String { return .error(object["token"] as? String, error) }
        guard let token = object["token"] as? String else { return nil }
        if object["done"] as? Bool == true { return .done(token) }
        guard let rate = object["rate"] as? Double, rate.isFinite, rate > 0, rate <= 192000,
              let encoded = object["audio"] as? String, encoded.utf8.count <= 24 * 1024 * 1024,
              let pcm = Data(base64Encoded: encoded), pcm.count >= 2, pcm.count <= 16 * 1024 * 1024,
              let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1),
              let audio = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(pcm.count / 2)),
              let channel = audio.floatChannelData?[0] else { return nil }
        audio.frameLength = audio.frameCapacity
        pcm.withUnsafeBytes { raw in
            for index in 0..<Int(audio.frameLength) {
                let sample = raw.loadUnaligned(fromByteOffset: index * 2, as: Int16.self)
                channel[index] = Float(Int16(littleEndian: sample)) / 32768
            }
        }
        return .audio(token, audio)
    }
}

/// A warm Piper process, with bounded streaming phrases and cancellable playback.
@MainActor final class SpeechPlayback {
    var onModels: (([String]) -> Void)?
    var onStatus: ((String) -> Void)?
    private(set) var models = [String]()
    private var process: Process?, input: Pipe?, output: Pipe?
    private var token = UUID().uuidString
    private let writeQueue = DispatchQueue(label: "kai.speech-write", qos: .userInitiated)
    private var engine:AVAudioEngine?,player:AVAudioPlayerNode?
    var playbackInitialized:Bool{engine != nil || player != nil}
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
        let decoder = SpeechReplyDecoder()
        let decodeQueue = DispatchQueue(label: "kai.speech-decode", qos: .userInitiated)
        outgoing.fileHandleForReading.readabilityHandler = { [weak self, weak task] handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil; return }
            decodeQueue.async { [weak self, weak task] in
                let replies = decoder.receive(data)
                guard !replies.isEmpty else { return }
                DispatchQueue.main.async { [weak self, weak task] in
                    guard let self, let task, self.process === task else { return }
                    for reply in replies { self.accept(reply) }
                }
            }
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
        guard warm, inFlight == 0, queuedBuffers < 2, !phrases.isEmpty, let input, let process else { return }
        let phrase = phrases.removeFirst(), requestToken = token, handle = input.fileHandleForWriting
        inFlight = 1
        requestStarted = Date()
        writeQueue.async { [weak self, weak process] in
            do {
                let data = try JSONSerialization.data(withJSONObject: ["text": phrase, "token": requestToken])
                try handle.write(contentsOf: data + Data([10]))
            } catch {
                DispatchQueue.main.async { [weak self, weak process] in
                    guard let self, let process, self.process === process, self.token == requestToken else { return }
                    self.inFlight = 0
                    self.warm = false
                    self.onStatus?("Local voice connection closed")
                }
            }
        }
    }

    private func accept(_ reply: SpeechReplyDecoder.Reply) {
        switch reply {
        case .ready:
            warm = true
            onStatus?("Local voice ready")
            pump()
        case .error(let requestToken, let message):
            guard requestToken == nil || requestToken == token else { return }
            inFlight = 0
            onStatus?(message)
        case .done(let requestToken):
            guard requestToken == token else { return }
            inFlight = 0
            pump()
        case .audio(let requestToken, let audio):
            guard requestToken == token else { return }
            play(audio)
            onStatus?("Voice first audio: \(Int(Date().timeIntervalSince(requestStarted) * 1000)) ms")
        }
    }

    private func play(_ audio: AVAudioPCMBuffer) {
        if engine == nil { engine = AVAudioEngine(); player = AVAudioPlayerNode() }
        guard let engine, let player else { return }
        let next = audio.format
        if format?.sampleRate != next.sampleRate {
            engine.stop()
            if player.engine != nil { engine.detach(player) }
            engine.attach(player)
            engine.connect(player, to: engine.mainMixerNode, format: next)
            format = next
        }
        do {if !engine.isRunning{try engine.start()};if !player.isPlaying{player.play()}}catch{onStatus?(error.localizedDescription);inFlight=0;return}
        // Generate the next phrase as soon as this one is queued, while playback runs.
        let playbackToken=token;queuedBuffers += 1
        player.scheduleBuffer(audio,completionCallbackType:.dataPlayedBack){ [weak self] _ in
            DispatchQueue.main.async{ [weak self] in guard let self,self.token == playbackToken else{return};self.queuedBuffers=max(0,self.queuedBuffers-1);self.pump()}
        }
    }
    func cancelAudio() {token=UUID().uuidString;player?.stop();queuedBuffers=0;pending="";lastQueuedPhrase=nil;phrases.removeAll();inFlight=0;flush?.cancel()}
    func stop() {cancelAudio();engine?.stop();player=nil;engine=nil;format=nil;warm=false;output?.fileHandleForReading.readabilityHandler=nil;process?.terminationHandler=nil;if process?.isRunning == true{process?.terminate()};process=nil;input=nil;output=nil;seen.removeAll();thread=nil}
}
