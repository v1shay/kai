import Foundation

struct KaiProject: Decodable { let path: String; let name: String }
struct KaiChat: Decodable { let id: String; let name: String; let status: String }
struct KaiLine: Decodable {
    let text: String
    let style: String
    let mathEnabled: Bool?
    init(text: String, style: String, mathEnabled: Bool? = nil) {
        self.text = text; self.style = style; self.mathEnabled = mathEnabled
    }
}
struct KaiMessage: Decodable { let id: String; let kind: String; let lines: [KaiLine] }
struct KaiFile: Decodable { let path: String; let name: String; let depth: Int; let directory: Bool; let collapsed: Bool }
struct KaiActivity: Decodable { let scene: String; let text: String }
struct KaiActiveTask: Decodable { let id: String; let name: String; let scene: String; let text: String }
struct KaiState: Decodable {
    let project: String
    let projects: [KaiProject]
    let chats: [KaiChat]
    let standaloneChats: [KaiChat]
    let standalone: Bool
    let threadId: String?
    let threadName: String
    let history: [KaiMessage]
    let files: [KaiFile]?
    let readOnly: Bool
    let activeTurnId: String?
    let approval: Bool
    let approvalThreadId: String?
    let activity: KaiActivity
    let activeTasks: [KaiActiveTask]
    let focusThreadId: String?
    let notice: String
    let attachments: [String]
    let completedRequestId: String?
    let rateLimitRemainingPercent: Int?
}

@MainActor final class KaiBridge {
    var onState: ((KaiState) -> Void)?
    var onError: ((String) -> Void)?
    private var process: Process?
    private var input: Pipe?, output: Pipe?, errors: Pipe?
    private var buffer = Data()
    private var generation = UUID()
    private var stopping = false
    private var failures = 0
    private var retryWork: DispatchWorkItem?
    private var lastStderr = ""

    func start() {
        guard !stopping,process?.isRunning != true else { return }
        retryWork?.cancel(); retryWork = nil
        launch()
    }

    private func launch() {
        generation = UUID()
        let token = generation
        buffer.removeAll(keepingCapacity: true)
        lastStderr = ""
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
        let extra=["/opt/homebrew/bin","/usr/local/bin",FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin").path]
        environment["PATH"]=(extra+[environment["PATH"] ?? "/usr/bin:/bin"]).joined(separator:":")
        process.environment=environment
        process.standardInput = input
        process.standardOutput = output
        process.standardError = errors
        output.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let bytes = handle.availableData
            DispatchQueue.main.async { [weak self] in
                guard let self,self.generation == token else { return }
                self.receive(bytes)
            }
        }
        errors.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let message = String(decoding: handle.availableData, as: UTF8.self)
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

    private func receive(_ bytes: Data) {
        guard !bytes.isEmpty else { return }
        // Scan only the new chunk. Searching the accumulated Data for every pipe
        // read made a large JSON state quadratic and pinned the UI thread.
        bytes.withUnsafeBytes { raw in
            let chars = raw.bindMemory(to: UInt8.self)
            var start = 0
            for index in chars.indices where chars[index] == 10 {
                buffer.append(contentsOf: chars[start..<index])
                if let state = try? JSONDecoder().decode(KaiState.self, from: buffer) { failures = 0; onState?(state) }
                buffer.removeAll(keepingCapacity: true)
                start = index + 1
            }
            if start < chars.count { buffer.append(contentsOf: chars[start..<chars.count]) }
        }
    }

    func send(_ action: String, _ fields: [String: Any] = [:]) {
        guard process?.isRunning == true,let input else { onError?("Codex is reconnecting; try again shortly"); return }
        var request = fields
        request["action"] = action
        guard let bytes = try? JSONSerialization.data(withJSONObject: request),
              let newline = "\n".data(using: .utf8) else { return }
        do { try input.fileHandleForWriting.write(contentsOf: bytes + newline) }
        catch { onError?("Codex is reconnecting; try again shortly") }
    }

    func stop() {
        stopping = true
        retryWork?.cancel(); retryWork = nil
        output?.fileHandleForReading.readabilityHandler = nil
        errors?.fileHandleForReading.readabilityHandler = nil
        if process?.isRunning == true { process?.terminate() }
    }
}
