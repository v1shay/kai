import Foundation
import Darwin

struct KaiProject: Decodable, Equatable { let path: String; let name: String }
struct KaiChat: Decodable, Equatable { let id: String; let name: String; let status: String }
struct KaiLine: Decodable, Equatable {
    let text: String
    let style: String
    let mathEnabled: Bool?
    init(text: String, style: String, mathEnabled: Bool? = nil) {
        self.text = text; self.style = style; self.mathEnabled = mathEnabled
    }
}
struct KaiMessage: Decodable, Equatable { let id: String; let kind: String; let lines: [KaiLine]; let speechText: String?; let activityIcon: String? }
struct KaiFile: Decodable { let path: String; let name: String; let depth: Int; let directory: Bool; let collapsed: Bool }
struct KaiActivity: Decodable { let scene: String; let text: String }
struct KaiActiveTask: Decodable, Equatable { let id: String; let name: String; let scene: String; let text: String }
struct KaiReasoningOption: Decodable { let reasoningEffort: String }
struct KaiModel: Decodable { let model: String; let displayName: String; let supportedReasoningEfforts: [KaiReasoningOption] }
struct KaiCodeConnectionState:Decodable {let connected:Bool;let enabled:Bool;let error:String?}
struct KaiState: Decodable {
    let voice:KaiVoiceState?
    let sessionActivity:KaiActivity?
    let codeAcceptedRequestId:String?
    let codeSessionActivity:KaiActivity?
    let codeChats:[KaiChat]?
    let codeConnection:KaiCodeConnectionState?
    let canGoBack: Bool?
    let canGoForward: Bool?
    let models: [KaiModel]?
    let selectedModel: String?
    let reasoningEffort: String?
    let project: String
    let projects: [KaiProject]
    let projectsLoading: Bool?
    let chats: [KaiChat]
    let standaloneChats: [KaiChat]
    let standalone: Bool
    let threadId: String?
    let threadName: String
    let history: [KaiMessage]
    let historyHasMore:Bool?
    let files: [KaiFile]?
    let readOnly: Bool
    let activeTurnId: String?
    let approval: Bool
    let approvalThreadId: String?
    let approvalRequestId:String?
    let approvalSummary:String?
    let activity: KaiActivity
    let activeTasks: [KaiActiveTask]
    let focusThreadId: String?
    let notice: String
    let attachments: [String]
    let completedRequestId: String?
    let completedRequestError:String?
    let rateLimitRemainingPercent: Int?

    var viewedActivity:KaiActivity {
        if let selected=sessionActivity ?? codeSessionActivity{return selected}
        if let selected=threadId,let task=activeTasks.first(where:{$0.id == selected}){return KaiActivity(scene:task.scene,text:task.text)}
        if let focus=focusThreadId,focus != threadId{return KaiActivity(scene:"off",text:"")}
        return activity
    }
}

/// Used exclusively on the bridge's serial decode queue.
final class KaiStateStreamDecoder {
    private var buffer=Data()
    private var dropping=false
    private let maximumLineBytes:Int
    init(maximumLineBytes:Int=64*1024*1024){self.maximumLineBytes=maximumLineBytes}
    func receive(_ bytes:Data)->[KaiState] {receiveEvents(bytes).compactMap{if case .state(let state)=$0{return state};return nil}}
    func receiveEvents(_ bytes:Data)->[KaiBridgeEvent] {
        var states=[KaiBridgeEvent]()
        bytes.withUnsafeBytes { raw in
            let chars=raw.bindMemory(to:UInt8.self)
            var start=0
            func append(_ end:Int){
                guard !dropping else{return}
                if buffer.count+end-start > maximumLineBytes{buffer.removeAll(keepingCapacity:false);dropping=true}
                else{buffer.append(contentsOf:chars[start..<end])}
            }
            for index in chars.indices where chars[index] == 10 {
                append(index)
                if !dropping,let event=KaiBridgeEvent.decode(buffer){states.append(event)}
                buffer.removeAll(keepingCapacity:buffer.count<=1024*1024);dropping=false;start=index+1
            }
            if start < chars.count {append(chars.count)}
        }
        return states
    }
}

enum KaiBridgeEvent {
    case state(KaiState),audio(String,KaiVoiceAudio),reset(String),sdp(String,String)
    static func decode(_ data:Data)->KaiBridgeEvent? {
        struct Envelope:Decodable {let type:String?;let sessionId:String?;let audio:KaiVoiceAudio?;let sdp:String?}
        // Reuse a single decoder's parsed JSON for the envelope and payload.
        struct Packet:Decodable {
            let event:KaiBridgeEvent?
            init(from decoder:Decoder)throws {
                let envelope=try Envelope(from:decoder)
                switch envelope.type {
                case "voiceAudio":event=envelope.sessionId.flatMap{id in envelope.audio.map{.audio(id,$0)}}
                case "voiceSDP":event=envelope.sessionId.flatMap{id in envelope.sdp.flatMap{$0.utf8.count<=128*1024 ? .sdp(id,$0):nil}}
                case "voiceReset":event=envelope.sessionId.map{.reset($0)}
                case nil,"state":event = .state(try KaiState(from:decoder))
                default:event=nil
                }
            }
        }
        return (try? JSONDecoder().decode(Packet.self,from:data))?.event
    }
}

@MainActor final class KaiBridge {
    var onState: ((KaiState) -> Void)?
    var onVoiceSDP:((String,String)->Void)?
    var onVoiceAudio:((String,KaiVoiceAudio)->Void)?
    var onVoiceReset:((String)->Void)?
    var onError: ((String) -> Void)?
    private var process: Process?
    private var input: Pipe?, output: Pipe?, errors: Pipe?
    private let writeQueue=DispatchQueue(label:"kai.bridge-write",qos:.userInitiated)
    private let audioWrites=DispatchSemaphore(value:16)
    private let decodeQueue=DispatchQueue(label:"kai.state-decode",qos:.userInitiated)
    private var selectedThreadID:String?
    private var generation = UUID()
    private var stopping = false
    private var failures = 0
    private var launchedAt=Date.distantPast
    private var retryWork: DispatchWorkItem?
    private var lastStderr = ""

    func start() {
        guard !stopping,process?.isRunning != true else { return }
        retryWork?.cancel(); retryWork = nil
        launch()
    }

    private func launch() {
        signal(SIGPIPE,SIG_IGN)
        generation = UUID()
        let token = generation
        let decoder=KaiStateStreamDecoder()
        let decodeQueue=self.decodeQueue
        lastStderr = "";launchedAt=Date()
        let sourceRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let resourceRoot = Bundle.main.resourceURL
        let root = resourceRoot.flatMap {
            FileManager.default.fileExists(atPath: $0.appendingPathComponent("kai_bridge.py").path) ? $0 : nil
        } ?? sourceRoot
        let script = root.appendingPathComponent("kai_bridge.py")
        let saved = UserDefaults.standard.string(forKey: "kaiProject").map(URL.init(fileURLWithPath:))
        let cwd = [saved, sourceRoot].compactMap { $0 }.first {
            FileManager.default.fileExists(atPath: $0.appendingPathComponent("Package.swift").path)
            || FileManager.default.fileExists(atPath: $0.path, isDirectory: nil)
        } ?? FileManager.default.homeDirectoryForCurrentUser
        let process = Process(),input = Pipe(),output = Pipe(),errors = Pipe()
        self.process = process;self.input = input;self.output = output;self.errors = errors
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", script.path, "--cwd", cwd.path]
        process.currentDirectoryURL = root
        var environment=ProcessInfo.processInfo.environment
        // Imports must not add bytecode files inside the signed app bundle.
        environment["PYTHONDONTWRITEBYTECODE"]="1"
        if let selectedThreadID,selectedThreadID.hasPrefix("code:"){environment["KAI_CODE_SELECTED"]=selectedThreadID}
        environment["KAI_CODE_ENABLED"]=(UserDefaults.standard.object(forKey:"connectCode") as? Bool ?? true) ? "1":"0"
        let extra=["/opt/homebrew/bin","/usr/local/bin",FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin").path]
        environment["PATH"]=(extra+[environment["PATH"] ?? "/usr/bin:/bin"]).joined(separator:":")
        process.environment=environment
        process.standardInput = input
        process.standardOutput = output
        process.standardError = errors
        output.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let bytes = handle.availableData
            if bytes.isEmpty {handle.readabilityHandler=nil;return}
            guard let self else{return}
            decodeQueue.async {
                let states=decoder.receiveEvents(bytes)
                guard !states.isEmpty else{return}
                DispatchQueue.main.async { [weak self] in
                    guard let self,self.generation == token,!self.stopping else{return}
                    if Date().timeIntervalSince(self.launchedAt) >= 60{self.failures=0}
                    for event in states {
                        switch event {
                        case .state(let state):self.selectedThreadID=state.threadId;self.onState?(state)
                        case .audio(let id,let audio):self.onVoiceAudio?(id,audio)
                        case .reset(let id):self.onVoiceReset?(id)
                        case .sdp(let id,let sdp):self.onVoiceSDP?(id,sdp)
                        }
                    }
                }
            }
        }
        errors.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data=handle.availableData
            if data.isEmpty{handle.readabilityHandler=nil;return}
            let message = String(decoding:data, as: UTF8.self)
            if !message.isEmpty { DispatchQueue.main.async { [weak self] in
                guard let self,self.generation == token else { return }
                self.lastStderr=String(message.trimmingCharacters(in:.whitespacesAndNewlines).suffix(300))
            } }
        }
        process.terminationHandler = { [weak self] ended in
            DispatchQueue.main.async { [weak self] in
                guard let self,self.generation == token else { return }
                self.connectionStopped("Codex connection stopped (\(ended.terminationStatus))")
            }
        }
        do { try process.run() }
        catch { connectionStopped("Could not start Codex connection: \(error.localizedDescription)") }
    }

    private func connectionStopped(_ reason: String) {
        output?.fileHandleForReading.readabilityHandler = nil
        errors?.fileHandleForReading.readabilityHandler = nil
        process = nil; input = nil; output = nil; errors = nil
        guard !stopping else { return }
        failures += 1
        let delay = min(30.0, Double(1 << min(failures - 1, 5)))
        let detail = lastStderr.isEmpty ? reason : lastStderr
        onError?("Codex disconnected; reconnecting in \(Int(delay))s. \(detail)")
        let work = DispatchWorkItem { [weak self] in
            guard let self,!self.stopping else { return }
            self.retryWork = nil
            self.launch()
        }
        retryWork = work
        DispatchQueue.main.asyncAfter(deadline:.now()+delay,execute:work)
    }

    func send(_ action: String, _ fields: [String: Any] = [:]) {
        guard process?.isRunning == true,let input else { onError?("Kai is reconnecting; your prompt is preserved"); return }
        var request = fields
        for (key,value) in DefaultModelPreferences(defaults:.standard).fields(action:action){request[key]=value}
        if request["targetThreadId"] == nil,let selectedThreadID{request["targetThreadId"]=selectedThreadID}
        request["action"] = action
        guard let bytes = try? JSONSerialization.data(withJSONObject: request),
              let newline = "\n".data(using: .utf8) else { return }
        let audioWrites=self.audioWrites,isAudio=action == "voice_audio"
        if isAudio,audioWrites.wait(timeout:.now()) != .success{onError?("Voice audio connection fell behind; start voice again");return}
        let token=generation,handle=input.fileHandleForWriting
        writeQueue.async { [weak self] in
            defer{if isAudio{audioWrites.signal()}}
            do {try handle.write(contentsOf:bytes+newline)}
            catch {DispatchQueue.main.async { [weak self] in
                guard let self,self.generation == token,!self.stopping else{return}
                self.onError?("Kai is reconnecting; your prompt is preserved")
            }}
        }
    }

    func stop() {
        stopping = true
        retryWork?.cancel(); retryWork = nil
        output?.fileHandleForReading.readabilityHandler = nil
        errors?.fileHandleForReading.readabilityHandler = nil
        if process?.isRunning == true { process?.terminate() }
    }
}
