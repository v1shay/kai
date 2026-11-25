import AppKit
import SwiftUI
import ScreenCaptureKit
import UniformTypeIdentifiers
import Quartz
import CoreImage

private struct KaiRemotePeer: Codable, Hashable, Identifiable {
    var id: String { node }
    let node: String
    let device: String
    let name: String
    let account: String?
    let build: Int?
    let compatible: Bool
    let allowed: Bool
    let online: Bool
    let update: Bool?
    let control: Bool?
    let capabilities: [String]?
    let fileProject: String?
    let terminal: Bool?
    var transport: String? = nil
}

private struct KaiRemoteApproval: Codable, Identifiable {
    var id: String { peer.node }
    let peer: KaiRemotePeer
}

private struct KaiRemoteUpdate: Codable {
    let state: String
    let build: Int?
    let message: String?
    var remote: Bool? = nil
}

private struct KaiRemoteScreenshot: Codable, Identifiable {
    let id: String
    let peer: KaiRemotePeer
}

private struct KaiRemoteStatus: Codable {
    let device: String
    let enabled: Bool
    var systemTailnet: Bool? = nil
    let network: String
    let authUrl: URL?
    let error: String?
    let requests: [KaiRemoteApproval]
    let screenshots: [KaiRemoteScreenshot]
    let trusted: [KaiRemotePeer]
    let update: KaiRemoteUpdate
}

private struct KaiRemoteWorkspace: Codable {
    struct Project: Codable, Identifiable {
        var id: String { path }
        let path: String
        let name: String
    }
    struct Chat: Codable, Identifiable {
        let id: String
        let title: String
        let agent: String
        let project: String
        let updatedAt: Int64?
    }
    let projects: [Project]
    let chats: [Chat]
    let nextOffset: Int?
}

private struct KaiRemoteMessage: Codable, Identifiable {
    let id: Int64
    let role: String
    let text: String
    let truncated: Bool
}

private struct KaiRemoteActivity: Codable, Identifiable {
    var id: String { chat }
    let chat: String
    let sequence: Int64
    let status: String
    let tail: String?
}

private struct KaiWorkspaceCommand: Codable, Equatable {
    let action: String
    let requestId: String
    let chatId: String?
    let project: String?
    let agent: String?
    let prompt: String?
    let name: String?
    var program: String? = nil
    var runId: String? = nil
    var answer: KaiRemoteDecision? = nil
}

private struct KaiRemoteDecision: Codable, Equatable {
    let approvalRequestId: String
    var choice: String?
    var answers: [String: [String]]?
}

private struct KaiRemoteRunControls: Decodable {
    struct Run: Decodable {
        let runId: String?
        let status: String
    }
    struct Option: Decodable, Identifiable {
        let id: String
        let label: String
    }
    struct Question: Decodable, Identifiable {
        let id: String
        let question: String
        let options: [Option]
        let secret: Bool
        let multiple: Bool
        let required: Bool
    }
    struct Request: Decodable {
        struct Kind: Decodable {
            struct Approval: Decodable { let choices: [Option] }
            struct Questions: Decodable { let questions: [Question] }
            let approval: Approval?
            let questions: Questions?

            init(from decoder: Decoder) throws {
                guard let object = try? decoder.container(keyedBy: CodingKeys.self) else {
                    approval = nil
                    questions = nil
                    return
                }
                approval = try object.decodeIfPresent(Approval.self, forKey: .approval)
                questions = try object.decodeIfPresent(Questions.self, forKey: .questions)
            }
            private enum CodingKeys: String, CodingKey {
                case approval = "Approval", questions = "Questions"
            }
        }
        let title: String
        let detail: String
        let requestKind: Kind
    }
    struct Approval: Decodable, Identifiable {
        var id: String { approvalRequestId }
        let approvalRequestId: String
        let request: Request
    }
    let tasks: [Run]
    let approvals: [Approval]
}

private struct KaiRemoteArtifact: Decodable, Identifiable {
    let id: String
    let title: String
    let kind: String
    let content: String?
}

private struct KaiRemoteArtifactList: Decodable {
    let artifacts: [KaiRemoteArtifact]
    let nextOffset: Int?
}

private struct KaiRemoteFileDiff: Decodable {
    let diff: String
}

private struct KaiCommandResult: Decodable {
    let error: String?
    let state: String?
    let result: Detail?
    struct Detail: Decodable { let error: String? }
}

private struct KaiRemoteTerminal: Decodable, Identifiable {
    let id: String
    let program: String
}

private struct KaiRemoteDirectory: Decodable {
    struct Entry: Decodable, Identifiable {
        var id: String { name }
        let name: String
        let kind: String
        let bytes: Int64
    }
    let entries: [Entry]
    let next: String?
    let revision: String
}

private struct KaiRemoteFilePreview: Decodable {
    var text: String?
    var revision: String
    let size: Int64
    let contentType: String
    let editable: Bool
}

private struct KaiFileReceipt: Decodable {
    let revision: String
}

private struct KaiRemoteCursor: Codable {
    let owner: String
    let offset: Int64
}

private struct KaiRemoteMetadataDelta: Decodable {
    let projects: [KaiRemoteWorkspace.Project]
    let chats: [KaiRemoteWorkspace.Chat]
    let removedProjects: [String]
    let removedChats: [String]
    let nextOffset: Int?
}

private struct KaiRemotePairingCode: Decodable, Identifiable {
    var id: String { code }
    let code: String
    let link: String

    func image() -> NSImage? {
        guard let filter = CIFilter(name: "CIQRCodeGenerator"),
              let data = link.data(using: .utf8) else { return nil }
        filter.setValue(data, forKey: "inputMessage")
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 6, y: 6)),
              let bitmap = CIContext().createCGImage(output, from: output.extent) else { return nil }
        return NSImage(cgImage: bitmap, size: CGSize(width: bitmap.width, height: bitmap.height))
    }
}

private struct KaiRemoteStreamFrame: Decodable {
    let type: String
    let workspace: KaiRemoteWorkspace?
    let delta: KaiRemoteMetadataDelta?
    let directory: KaiRemoteDirectory?
    let path: String?
    let activity: [KaiRemoteActivity]?
    let chat: String?
    let cursor: KaiRemoteCursor?
}

private struct KaiRemoteRequest: Encodable {
    let action: String
    var node: String? = nil
    var device: String? = nil
    var update: Bool? = nil
    var control: Bool? = nil
    var terminal: Bool? = nil
    var command: KaiWorkspaceCommand? = nil
    var project: String? = nil
    var chat: String? = nil
    var cursor: String? = nil
    var screenshot: String? = nil
    var path: String? = nil
    var file: String? = nil
    var text: String? = nil
    var revision: String? = nil
    var streamCursor: KaiRemoteCursor? = nil
    var code: String? = nil
}

private struct KaiRemoteAck: Decodable {}

@MainActor
final class KaiRemoteDevices: NSObject, ObservableObject, NSWindowDelegate {
    @Published private(set) var menuTitle = "Remote Devices…"
    @Published fileprivate var status: KaiRemoteStatus?
    @Published fileprivate var peers: [KaiRemotePeer] = []
    @Published fileprivate var workspace: KaiRemoteWorkspace?
    @Published fileprivate var messages: [KaiRemoteMessage] = []
    @Published fileprivate var selected: KaiRemotePeer?
    @Published fileprivate var selectedChat: KaiRemoteWorkspace.Chat?
    @Published fileprivate var project: String?
    @Published fileprivate var remoteUpdate: KaiRemoteUpdate?
    @Published fileprivate var activity: [KaiRemoteActivity] = []
    @Published fileprivate var requestedScreenshot: String?
    @Published fileprivate var screenshotImage: NSImage?
    @Published fileprivate var localProjects: [KaiRemoteWorkspace.Project] = []
    @Published fileprivate var pairingCode: KaiRemotePairingCode?
    @Published fileprivate var runControls: KaiRemoteRunControls?
    @Published fileprivate var showRunControls = false
    @Published fileprivate var artifactPanel = false
    @Published fileprivate var artifacts: KaiRemoteArtifactList?
    @Published fileprivate var artifact: KaiRemoteArtifact?
    @Published fileprivate var fileDiff: String?
    @Published fileprivate var searchResults: [KaiRemoteDirectory.Entry]?
    @Published fileprivate var fileSearch = ""


    @Published fileprivate var directory: KaiRemoteDirectory?
    @Published fileprivate var directoryPath: String?
    @Published fileprivate var filePath: String?
    @Published fileprivate var filePreview: KaiRemoteFilePreview?
    @Published fileprivate var fileDraft = ""
    @Published fileprivate var richPreview: URL?
    @Published fileprivate var terminalsOpen = false
    @Published fileprivate var terminals: [KaiRemoteTerminal] = []
    @Published fileprivate var attachedTerminal: KaiRemoteTerminal?
    private var terminalRequest: KaiWorkspaceCommand?

    private var richPreviewDirectory: URL?
    private var filesGeneration = 0
    @Published fileprivate var liveTools: [String] = []
    private var streamOutput: Process?
    private var streamGeneration = 0
    private var streamActive = false
    private var streamBackoff = 1.0
    private var journals: [String: (KaiRemoteCursor, String)] = [:]


    private var cache: [String: [KaiRemoteMessage]] = [:]
    private var cacheOrder: [String] = []
    @Published fileprivate var live: KaiRemoteActivity?
    @Published fileprivate var draft = ""
    private var submitted: KaiWorkspaceCommand?
    private var ensureHost: (() -> Void)?
    private var hostRequested = false
    @Published fileprivate var issue = ""
    @Published fileprivate var busy = false
    private let queue = DispatchQueue(label: "kai.remote.devices", qos: .utility, attributes: .concurrent)
    private var menuItem: NSMenuItem?
    private var updateItem: NSMenuItem?
    private var statusTask: Task<Void, Never>?
    private var selectionGeneration = 0
    private var lastDiscover = Date.distantPast

    private var restartRequested = false
    private var relaunchStarted = false
    private let runningBuild = Int(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "")
    private var quit: (() -> Void)?

    static var available: Bool {
        FileManager.default.isExecutableFile(atPath: executable.path)
    }

    private static var executable: URL {
        Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/kai-server")
    }

    func start(menu: NSMenuItem?, update: NSMenuItem, ensureHost: @escaping () -> Void, quit: @escaping () -> Void) {
        menuItem = menu
        updateItem = update
        self.quit = quit
        self.ensureHost = ensureHost
        guard CommandLine.arguments.count == 1 else { return }
        statusTask = Task { [weak self] in
            guard let self else { return }
            do { _ = try await run(["bootstrap"]) }
            catch { issue = error.localizedDescription; return }
            while !Task.isCancelled {
                await refreshStatus()
                try? await Task.sleep(nanoseconds: 15_000_000_000)
            }
        }
    }

    private func stopStream() {
        streamGeneration += 1
        streamActive = false
        if let process = streamOutput, process.isRunning { process.terminate() }
        streamOutput = nil
    }

    private func restartStream(retrying: Bool = false) {
        stopStream()
        guard let peer = selected, peer.online, peer.allowed,
              peer.capabilities?.contains("workspace.stream") == true else { return }
        if !retrying { streamBackoff = 1 }
        let generation = streamGeneration
        let chat = directoryPath == nil && !terminalsOpen ? selectedChat?.id : nil
        let key = peer.device + ":" + (chat ?? "")
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let input = try? encoder.encode(KaiRemoteRequest(action: "stream", node: peer.node, device: peer.device,
                                                               chat: chat, file: directoryPath, streamCursor: journals[key]?.0)) else { return }
        if let chat, let existing = journals[key], !existing.1.isEmpty {
            live = KaiRemoteActivity(chat: chat, sequence: existing.0.offset, status: "Running", tail: existing.1)
        }
        let process = Process()
        let pipe = Pipe(), stdin = Pipe()
        process.executableURL = Self.executable
        process.arguments = ["stream"]
        process.standardInput = stdin
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            try stdin.fileHandleForWriting.write(contentsOf: input)
            try stdin.fileHandleForWriting.close()
        } catch {
            if process.isRunning { process.terminate() }
            return
        }
        streamOutput = process
        queue.async { [weak self] in
            var pending = Data()
            let handle = pipe.fileHandleForReading
            while let data = try? handle.read(upToCount: 32_768), !data.isEmpty {
                pending.append(data)
                if pending.count > 8 * 1024 * 1024 { break }
                while let end = pending.firstIndex(of: 10) {
                    let line = Data(pending[..<end])
                    pending.removeSubrange(...end)
                    DispatchQueue.main.async { [weak self] in self?.consumeStream(line, generation: generation, peer: peer) }
                }
            }
            if process.isRunning { process.terminate() }
            DispatchQueue.main.async { [weak self] in self?.streamExited(generation) }
        }
    }

    private func streamExited(_ generation: Int) {
        guard streamGeneration == generation else { return }
        streamActive = false
        streamOutput = nil
        let wait = streamBackoff * Double.random(in: 0.85...1.2)
        streamBackoff = min(streamBackoff * 2, 20)
        Task {
            try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000))
            if streamGeneration == generation { restartStream(retrying: true) }
        }
    }

    private func consumeStream(_ line: Data, generation: Int, peer: KaiRemotePeer) {
        guard generation == streamGeneration else { return }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let frame = try? decoder.decode(KaiRemoteStreamFrame.self, from: line) else { return }
        streamActive = true
        streamBackoff = 1
        switch frame.type {
        case "revoked":
            stopStream()
            workspace = nil
            messages = []
            live = nil
            liveTools = []
            closeFiles()
            closeTerminals()
            journals = journals.filter { !$0.key.hasPrefix(peer.device + ":") }
            issue = "This Mac revoked your access."
        case "metadata":
            if project == nil, let value = frame.workspace {
                if let current = workspace, current.chats.count > 100 {
                    let ids = Set(value.chats.map(\.id))
                    workspace = KaiRemoteWorkspace(projects: value.projects,
                                                   chats: value.chats + current.chats.dropFirst(100).filter { !ids.contains($0.id) },
                                                   nextOffset: current.nextOffset)
                } else { workspace = value }
            } else if project != nil {
                Task { await loadWorkspace() }
            }
            if let selectedChat { Task { await loadMessages(selectedChat, peer: peer, quiet: true) } }
        case "metadata_delta":
            if project == nil, let delta = frame.delta, let previous = workspace {
                let removedProjects = Set(delta.removedProjects + delta.projects.map(\.id))
                let removedChats = Set(delta.removedChats + delta.chats.map(\.id))
                let nextProjects = previous.projects.filter { !removedProjects.contains($0.id) } + delta.projects
                let nextChats = previous.chats.filter { !removedChats.contains($0.id) } + delta.chats
                workspace = KaiRemoteWorkspace(projects: nextProjects.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending },
                                               chats: nextChats.sorted { ($0.updatedAt ?? 0) > ($1.updatedAt ?? 0) },
                                               nextOffset: previous.chats.count > 100 ? previous.nextOffset : delta.nextOffset)
            } else if project != nil {
                Task { await loadWorkspace() }
            }
            if let selectedChat { Task { await loadMessages(selectedChat, peer: peer, quiet: true) } }
        case "directory":
            if directoryPath == frame.path, let value = frame.directory, value.revision != directory?.revision {
                directory = value
            }
        case "activity":
            activity = frame.activity ?? []
        case "reset", "events":
            guard let chat = frame.chat, chat == selectedChat?.id, let cursor = frame.cursor else { return }
            let key = peer.device + ":" + chat
            var visible = frame.type == "reset" ? "" : journals[key]?.1 ?? ""
            if frame.type == "reset" { liveTools = [] }
            var status = "Running"
            let data = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any]
            if let events = data?["events"] as? [Any] {
                for event in events {
                    if let kind = event as? String, kind == "Finished" { status = "Finished"; continue }
                    guard let value = event as? [String: Any] else { continue }
                    if let reset = value["ReplaceText"] as? String ?? value["ReplacePreview"] as? String { visible = reset }
                    else if let text = value["Text"] as? String { visible += text }
                    if let caption = value["Status"] as? String { status = caption }
                    if let failed = value["Failed"] as? String { status = failed }
                    if let tool = value["WorkspaceTool"] as? [String: Any], let name = tool["tool"] as? String {
                        liveTools = Array((liveTools + [name]).suffix(12))
                    }
                }
            }
            if visible.utf8.count > 131_072 { visible = String(visible.suffix(32_768)) }
            if journals.count >= 12, journals[key] == nil, let oldest = journals.keys.first {
                journals.removeValue(forKey: oldest)
            }
            journals[key] = (cursor, visible)
            live = status == "Finished" ? nil : KaiRemoteActivity(chat: chat, sequence: cursor.offset, status: status, tail: visible)
        default: break
        }
    }

    private func run(_ arguments: [String], input: Data = Data()) async throws -> Data {
        let executable = Self.executable
        return try await withCheckedThrowingContinuation { continuation in
            queue.async {
                let process = Process()
                let stdout = Pipe(), stdin = Pipe(), stderr = Pipe()
                process.executableURL = executable
                process.arguments = arguments
                process.standardInput = stdin
                process.standardOutput = stdout
                process.standardError = stderr
                do {
                    try process.run()
                    try stdin.fileHandleForWriting.write(contentsOf: input)
                    try stdin.fileHandleForWriting.close()
                    let data = stdout.fileHandleForReading.readDataToEndOfFile()
                    let details = stderr.fileHandleForReading.readDataToEndOfFile()
                    process.waitUntilExit()
                    if process.terminationStatus != 0 {
                        let detail = String(decoding: details, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                        throw NSError(domain: "KaiRemote", code: Int(process.terminationStatus), userInfo: [
                            NSLocalizedDescriptionKey: detail.isEmpty ? "Kai Server is unavailable." : detail
                        ])
                    }
                    continuation.resume(returning: data)
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    private func call<T: Decodable>(_ request: KaiRemoteRequest, as result: T.Type) async throws -> T {
        let data = try await callData(request)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(result, from: data)
    }

    private func callData(_ request: KaiRemoteRequest) async throws -> Data {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return try await run(["call"], input: encoder.encode(request))
    }

    private func refreshStatus() async {
        do {
            status = try await call(KaiRemoteRequest(action: "status"), as: KaiRemoteStatus.self)
            let count = (status?.requests.count ?? 0) + (status?.screenshots.count ?? 0)
            menuTitle = count == 0 ? "Remote Devices…" : "Remote Devices… (\(count) pending)"
            menuItem?.title = menuTitle
            let updateReady = status?.update.state == "ready"
            updateItem?.isHidden = !updateReady
            if status?.trusted.contains(where: { $0.control == true }) == true, !hostRequested {
                hostRequested = true
                ensureHost?()
            }
            if status?.trusted.contains(where: { $0.control == true }) != true { hostRequested = false }
            if !restartRequested,
               let current = runningBuild, let result = status?.update,
               result.state == "installed", result.remote == true, let build = result.build, build > current {
                do { try relaunch() }
                catch { issue = error.localizedDescription }
            }
        } catch {
            if status == nil { issue = error.localizedDescription }
        }
    }

    fileprivate func perform(_ action: String, peer: KaiRemotePeer? = nil, update: Bool? = nil, control: Bool? = nil,
                             terminal: Bool? = nil, project: String? = nil, path: String? = nil) {
        Task {
            busy = true
            defer { busy = false }
            issue = ""
            do {
                _ = try await call(KaiRemoteRequest(action: action, node: peer?.node, device: peer?.device, update: update, control: control, terminal: terminal, project: project, path: path), as: KaiRemoteAck.self)
                await refreshStatus()
                if ["pair", "disconnect", "disable", "enable-system-tailnet", "disable-system-tailnet"].contains(action) {
                    await discover()
                }
                if action == "disable" {
                    selected = nil
                    workspace = nil
                    messages = []
                    closeFiles()
                    closeTerminals()
                    cache.removeAll()
                    cacheOrder.removeAll()
                }
                if action == "remote-update" { remoteUpdate = KaiRemoteUpdate(state: "checking", build: nil, message: "Remote update accepted.") }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func discover() async {
        lastDiscover = Date()
        do {
            let found = try await call(KaiRemoteRequest(action: "peers"), as: [KaiRemotePeer].self)
            peers = found
            if let previous = selected, let fresh = found.first(where: { $0.id == previous.id && $0.device == previous.device }) {
                selected = fresh
                if fresh.allowed && fresh.online && (!previous.allowed || !previous.online || fresh.transport != previous.transport) {
                    restartStream()
                }
                if fresh.allowed && workspace == nil && previous.allowed == false { await loadWorkspace() }
                if !fresh.allowed && fresh.online {
                    workspace = nil
                    selectedChat = nil
                    messages = []
                    closeFiles()
                    closeTerminals()
                    stopStream()
                    let prefix = fresh.device + ":"
                    cache.keys.filter { $0.hasPrefix(prefix) }.forEach { cache.removeValue(forKey: $0) }
                    cacheOrder.removeAll { $0.hasPrefix(prefix) }
                    journals = journals.filter { !$0.key.hasPrefix(prefix) }
                }
            }
        } catch {
            if peers.isEmpty { issue = error.localizedDescription }
        }
    }

    fileprivate func select(_ peer: KaiRemotePeer) {
        if selected?.id == peer.id && selected?.device == peer.device { return }
        selected = peer
        project = nil
        selectedChat = nil
        workspace = nil
        messages = []
        live = nil
        activity = []
        draft = ""
        submitted = nil
        remoteUpdate = nil
        requestedScreenshot = nil
        screenshotImage = nil
        closeFiles()
        closeTerminals()
        issue = ""
        selectionGeneration += 1
        liveTools = []
        restartStream()
        Task { if peer.allowed { await loadWorkspace() } }
    }

    fileprivate func showTerminals() {
        guard let peer = selected else { return }
        closeFiles()
        terminalsOpen = true
        restartStream()
        Task {
            do {
                let found = try await call(KaiRemoteRequest(action: "terminals", node: peer.node, device: peer.device),
                                           as: [KaiRemoteTerminal].self)
                if selected?.device == peer.device { terminals = found }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func closeTerminals() {
        attachedTerminal = nil
        terminals = []
        terminalsOpen = false
        terminalRequest = nil
    }

    fileprivate func startTerminal(program: String) {
        guard let peer = selected, !busy else { return }
        let command: KaiWorkspaceCommand
        if let pending = terminalRequest, pending.program == program { command = pending }
        else {
            command = KaiWorkspaceCommand(action: "terminal_start", requestId: UUID().uuidString,
                                          chatId: nil, project: nil, agent: nil, prompt: nil, name: nil, program: program)
        }
        terminalRequest = command
        Task {
            busy = true
            defer { busy = false }
            do {
                let item = try await call(KaiRemoteRequest(action: "terminal-start", node: peer.node, device: peer.device,
                                                           command: command), as: KaiRemoteTerminal.self)
                guard selected?.device == peer.device, terminalsOpen else { return }
                if !terminals.contains(where: { $0.id == item.id }) { terminals.append(item) }
                attachedTerminal = item
                terminalRequest = nil
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func terminalArguments(_ peer: KaiRemotePeer, terminal: KaiRemoteTerminal) -> [String] {
        [Self.executable.path, "terminal", peer.node, peer.device, terminal.id]
    }

    fileprivate func loadLocalProjects() async {
        do {
            localProjects = try await call(KaiRemoteRequest(action: "local-projects"), as: [KaiRemoteWorkspace.Project].self)
        } catch { issue = error.localizedDescription }
    }

    fileprivate func generatePairingCode() {
        Task {
            do {
                pairingCode = try await call(KaiRemoteRequest(action: "generate-code"), as: KaiRemotePairingCode.self)
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func pastePairingLink() {
        let alert = NSAlert()
        alert.messageText = "Pair with a Kai Link"
        alert.informativeText = "Paste the link from the host Mac. Both devices must be on the same tailnet."
        alert.addButton(withTitle: "Pair")
        alert.addButton(withTitle: "Cancel")
        let input = NSTextField(frame: NSRect(x: 0, y: 0, width: 370, height: 24))
        input.placeholderString = "kai://pair?…"
        alert.accessoryView = input
        alert.window.initialFirstResponder = input
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        guard let link = URLComponents(string: input.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)),
              link.scheme == "kai", link.host == "pair",
              let node = link.queryItems?.first(where: { $0.name == "node" })?.value,
              let device = link.queryItems?.first(where: { $0.name == "device" })?.value,
              let code = link.queryItems?.first(where: { $0.name == "code" })?.value else {
            issue = "That link is missing its Kai pairing details."
            return
        }
        Task {
            do {
                _ = try await call(KaiRemoteRequest(action: "redeem-code", node: node, device: device, code: code),
                                   as: KaiRemoteAck.self)
                await discover()
                if let peer = peers.first(where: { $0.node == node && $0.device == device }) { select(peer) }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func createProject() {
        guard let peer = selected, peer.control == true, let project, !busy else { return }
        let alert = NSAlert()
        alert.messageText = "Create Remote Project"
        alert.informativeText = "Kai will create a folder inside \(URL(fileURLWithPath: project).lastPathComponent) on \(peer.name)."
        alert.addButton(withTitle: "Create")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 300, height: 24))
        field.placeholderString = "Project name"
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        let command = KaiWorkspaceCommand(action: "create_project", requestId: UUID().uuidString,
                                          chatId: nil, project: project, agent: nil, prompt: nil, name: name)
        Task {
            busy = true
            defer { busy = false }
            do {
                let result = try await call(KaiRemoteRequest(action: "command", node: peer.node, device: peer.device,
                                                              command: command), as: KaiCommandResult.self)
                if let problem = result.error ?? result.result?.error { issue = problem }
                else { await loadWorkspace() }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func selectUpdateArchive() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.zip]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.message = "Choose a signed Kai update ZIP."
        if panel.runModal() == .OK, let url = panel.url {
            perform("import-update", path: url.path)
        }
    }

    fileprivate func closeFiles() {
        filesGeneration += 1
        directoryPath = nil
        directory = nil
        filePath = nil
        filePreview = nil
        fileDraft = ""
        fileDiff = nil
        searchResults = nil
        fileSearch = ""
        artifactPanel = false
        artifacts = nil
        artifact = nil
        closeRichPreview()
    }

    private func closeRichPreview() {
        richPreview = nil
        if let directory = richPreviewDirectory {
            try? FileManager.default.removeItem(at: directory)
            richPreviewDirectory = nil
        }
    }

    fileprivate func browseFiles(_ path: String = ".", more: Bool = false) {
        guard let peer = selected, let project = peer.fileProject else { return }
        let previous = more ? directory : nil
        if !more {
            closeFiles()
            directoryPath = path
            restartStream()
        }
        let generation = filesGeneration
        Task {
            do {
                let result = try await call(KaiRemoteRequest(action: "files", node: peer.node, device: peer.device,
                                                            project: project, cursor: previous?.next, file: path),
                                            as: KaiRemoteDirectory.self)
                guard generation == filesGeneration, directoryPath == path else { return }
                if let previous, result.revision == previous.revision {
                    let existing = Set(previous.entries.map(\.id))
                    directory = KaiRemoteDirectory(entries: previous.entries + result.entries.filter { !existing.contains($0.id) },
                                                   next: result.next, revision: result.revision)
                } else if previous != nil {
                    browseFiles(path)
                } else {
                    directory = result
                }
            } catch { if generation == filesGeneration { issue = error.localizedDescription } }
        }
    }

    fileprivate func openFile(_ entry: KaiRemoteDirectory.Entry, exact: Bool = false) {
        guard let parent = directoryPath, let peer = selected, let project = peer.fileProject else { return }
        let path = exact || parent == "." ? entry.name : parent + "/" + entry.name
        if entry.kind == "folder" {
            browseFiles(path)
            return
        }
        closeRichPreview()
        filesGeneration += 1
        let generation = filesGeneration
        filePath = path
        filePreview = nil
        fileDiff = nil
        fileDraft = ""
        artifactPanel = false
        Task {
            do {
                let result = try await call(KaiRemoteRequest(action: "file-preview", node: peer.node, device: peer.device,
                                                             project: project, file: path), as: KaiRemoteFilePreview.self)
                guard generation == filesGeneration else { return }
                filePreview = result
                fileDraft = result.text ?? ""
            } catch { if generation == filesGeneration { issue = error.localizedDescription } }
        }
    }

    fileprivate func searchProjectFiles() {
        guard let peer = selected, let project = peer.fileProject, let directory = directoryPath else { return }
        let query = fileSearch.trimmingCharacters(in: .whitespacesAndNewlines)
        if query.isEmpty { searchResults = nil; return }
        let generation = filesGeneration
        Task {
            do {
                let matches = try await call(KaiRemoteRequest(action: "file-search", node: peer.node, device: peer.device,
                                                              project: project, file: directory, text: query),
                                             as: [KaiRemoteDirectory.Entry].self)
                if generation == filesGeneration { searchResults = matches }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func uploadFile() {
        guard let peer = selected, let project = peer.fileProject, let directory = directoryPath, !busy else { return }
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.message = "Upload a new file into this folder on \(peer.name)."
        guard panel.runModal() == .OK, let local = panel.url else { return }
        let filename = directory == "." ? local.lastPathComponent : directory + "/" + local.lastPathComponent
        Task {
            busy = true
            defer { busy = false }
            do {
                _ = try await call(KaiRemoteRequest(action: "file-upload", node: peer.node, device: peer.device,
                                                    project: project, path: local.path, file: filename),
                                   as: KaiFileReceipt.self)
                if directoryPath == directory {
                    browseFiles(directory)
                    issue = "Uploaded \(local.lastPathComponent) to \(peer.name)"
                }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func loadFileDiff() {
        guard let peer = selected, let project = peer.fileProject, let filePath else { return }
        if fileDiff != nil { fileDiff = nil; return }
        let generation = filesGeneration
        Task {
            do {
                let result = try await call(KaiRemoteRequest(action: "file-diff", node: peer.node, device: peer.device,
                                                             project: project, file: filePath), as: KaiRemoteFileDiff.self)
                if generation == filesGeneration { fileDiff = result.diff }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func loadArtifacts(more: Bool = false) {
        guard let peer = selected, let project = peer.fileProject else { return }
        artifactPanel = true
        let previous = more ? artifacts : nil
        Task {
            do {
                let result = try await call(KaiRemoteRequest(action: "artifacts", node: peer.node, device: peer.device,
                                                              project: project, cursor: previous?.nextOffset.map(String.init)),
                                            as: KaiRemoteArtifactList.self)
                guard artifactPanel, selected?.device == peer.device else { return }
                artifacts = KaiRemoteArtifactList(artifacts: (previous?.artifacts ?? []) + result.artifacts,
                                                  nextOffset: result.nextOffset)
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func loadArtifact(_ item: KaiRemoteArtifact) {
        guard let peer = selected, let project = peer.fileProject else { return }
        Task {
            do {
                let result = try await call(KaiRemoteRequest(action: "artifacts", node: peer.node, device: peer.device,
                                                             project: project, file: item.id), as: KaiRemoteArtifact.self)
                if artifactPanel, selected?.device == peer.device { artifact = result }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func saveArtifactCopy() {
        guard let artifact, let content = artifact.content else { return }
        let kind = ["markdown": "md", "mermaid": "mmd", "html": "html"][artifact.kind] ?? "txt"
        let panel = NSSavePanel()
        panel.nameFieldStringValue = artifact.title.replacingOccurrences(of: "/", with: "-") + "." + kind
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do { try content.write(to: url, atomically: true, encoding: .utf8) }
        catch { issue = error.localizedDescription }
    }

    fileprivate func openRunControls() {
        showRunControls = true
        refreshRunControls()
    }

    fileprivate func refreshRunControls() {
        guard let peer = selected, let selectedChat else { return }
        Task {
            do {
                let value = try await call(KaiRemoteRequest(action: "run-controls", node: peer.node, device: peer.device,
                                                            chat: selectedChat.id), as: KaiRemoteRunControls.self)
                if thisChat(selectedChat, on: peer) { runControls = value }
            } catch { issue = error.localizedDescription }
        }
    }

    private func thisChat(_ chat: KaiRemoteWorkspace.Chat, on peer: KaiRemotePeer) -> Bool {
        selected?.device == peer.device && selectedChat?.id == chat.id
    }

    fileprivate func sendRunControl(_ action: String, runID: String? = nil, prompt: String? = nil,
                                    answer: KaiRemoteDecision? = nil) {
        guard let peer = selected, let selectedChat, !busy else { return }
        let command = KaiWorkspaceCommand(action: action, requestId: UUID().uuidString,
                                          chatId: selectedChat.id, project: selectedChat.project,
                                          agent: nil, prompt: prompt, name: nil, runId: runID, answer: answer)
        Task {
            busy = true
            defer { busy = false }
            do {
                let result = try await call(KaiRemoteRequest(action: "command", node: peer.node, device: peer.device,
                                                              command: command), as: KaiCommandResult.self)
                if let problem = result.error ?? result.result?.error { issue = problem }
                else { refreshRunControls() }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func saveFileEdit() {
        guard let peer = selected, let project = peer.fileProject,
              let path = filePath, var preview = filePreview, preview.editable, !busy else { return }
        let text = fileDraft
        guard text.utf8.count <= 10_000 else { issue = "Remote text edits are limited to 10 KB."; return }
        let generation = filesGeneration
        Task {
            busy = true
            defer { busy = false }
            do {
                let result = try await call(KaiRemoteRequest(action: "file-save", node: peer.node, device: peer.device,
                                                             project: project, file: path, text: text, revision: preview.revision),
                                            as: KaiFileReceipt.self)
                guard generation == filesGeneration else { return }
                preview.revision = result.revision
                preview.text = text
                filePreview = preview
                issue = "Saved on \(peer.name)"
            } catch { if generation == filesGeneration { issue = error.localizedDescription } }
        }
    }

    fileprivate func downloadFile(preview: Bool) {
        guard let peer = selected, let project = peer.fileProject, let path = filePath, !busy else { return }
        let basename = URL(fileURLWithPath: path).lastPathComponent
        var scratch: URL?
        let target: URL
        if preview {
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("kai-preview-\(UUID().uuidString)")
            do {
                try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: false,
                                                        attributes: [.posixPermissions: 0o700])
            } catch { issue = error.localizedDescription; return }
            scratch = dir
            target = dir.appendingPathComponent(basename)
        } else {
            let panel = NSSavePanel()
            panel.nameFieldStringValue = basename
            guard panel.runModal() == .OK, let url = panel.url else { return }
            target = url
        }
        let generation = filesGeneration
        Task {
            busy = true
            defer { busy = false }
            do {
                _ = try await call(KaiRemoteRequest(action: "file-download", node: peer.node, device: peer.device,
                                                    project: project, path: target.path, file: path),
                                   as: KaiFileReceipt.self)
                if preview, generation == filesGeneration {
                    closeRichPreview()
                    richPreviewDirectory = scratch
                    richPreview = target
                } else if let scratch { try? FileManager.default.removeItem(at: scratch) }
                if !preview { issue = "Downloaded \(basename)" }
            } catch {
                if let scratch { try? FileManager.default.removeItem(at: scratch) }
                issue = error.localizedDescription
            }
        }
    }

    fileprivate func selectProject(_ path: String?) {
        project = path
        selectionGeneration += 1
        selectedChat = nil
        messages = []
        restartStream()
        Task { await loadWorkspace() }
    }

    fileprivate func loadWorkspace(more: Bool = false) async {
        guard let peer = selected, peer.allowed else { return }
        let turn = selectionGeneration
        let offset = more ? workspace?.nextOffset : nil
        do {
            let value = try await call(KaiRemoteRequest(action: "workspace", node: peer.node, device: peer.device,
                                                       project: project, cursor: offset.map(String.init)), as: KaiRemoteWorkspace.self)
            guard turn == selectionGeneration else { return }
            if let previous = workspace, more {
                workspace = KaiRemoteWorkspace(projects: value.projects, chats: previous.chats + value.chats, nextOffset: value.nextOffset)
            } else { workspace = value }
        } catch { if turn == selectionGeneration { issue = error.localizedDescription } }
    }

    fileprivate func selectChat(_ chat: KaiRemoteWorkspace.Chat) {
        guard let peer = selected else { return }
        selectedChat = chat
        messages = cache[peer.device + ":" + chat.id] ?? []
        live = nil
        draft = ""
        submitted = nil
        liveTools = []
        restartStream()
        Task { await loadMessages(chat, peer: peer) }
    }

    fileprivate func returnToChats() {
        selectedChat = nil
        messages = []
        live = nil
        liveTools = []
        restartStream()
    }

    fileprivate func loadEarlier() {
        guard let peer = selected, let chat = selectedChat, let before = messages.first?.id else { return }
        Task { await loadMessages(chat, peer: peer, before: before) }
    }

    private func loadMessages(_ chat: KaiRemoteWorkspace.Chat, peer: KaiRemotePeer, before: Int64? = nil, quiet: Bool = false) async {
        do {
            let value = try await call(KaiRemoteRequest(action: "messages", node: peer.node, device: peer.device, chat: chat.id,
                                                       cursor: before.map(String.init)), as: [KaiRemoteMessage].self)
            guard selected?.device == peer.device, selectedChat?.id == chat.id else { return }
            if before != nil {
                let current = Set(messages.map(\.id))
                messages = value.filter { !current.contains($0.id) } + messages
            } else if let first = value.first?.id {
                messages = messages.filter { $0.id < first } + value
            }
            let key = peer.device + ":" + chat.id
            cache[key] = Array(messages.suffix(30))
            cacheOrder.removeAll { $0 == key }
            cacheOrder.append(key)
            if cacheOrder.count > 8 { cache.removeValue(forKey: cacheOrder.removeFirst()) }
        } catch { if !quiet, selectedChat?.id == chat.id { issue = error.localizedDescription } }
    }

    private func loadActivity(_ peer: KaiRemotePeer, chat: String?) async {
        do {
            let values = try await call(KaiRemoteRequest(action: "activity", node: peer.node, device: peer.device, chat: chat),
                                        as: [KaiRemoteActivity].self)
            guard selected?.device == peer.device, selectedChat?.id == chat else { return }
            if chat == nil { activity = values }
            else { live = values.first(where: { $0.status != "Finished" }) }
        } catch { }
    }

    fileprivate func sendDraft(agent: String? = nil) {
        let prompt = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !busy, !prompt.isEmpty, prompt.utf8.count <= 10_000, let peer = selected, peer.control == true else { return }
        let chat = selectedChat
        guard chat != nil || (agent != nil && project != nil) else { return }
        let current = KaiWorkspaceCommand(action: chat == nil ? "create_chat" : "prompt_chat", requestId: UUID().uuidString,
                                          chatId: chat?.id, project: chat?.project ?? project, agent: agent, prompt: prompt,
                                          name: chat == nil ? String(prompt.prefix(60)) : nil)
        let command: KaiWorkspaceCommand
        if let prior = submitted, prior.chatId == current.chatId, prior.project == current.project,
           prior.agent == current.agent, prior.prompt == current.prompt { command = prior } else { command = current }
        submitted = command
        Task {
            busy = true
            defer { busy = false }
            issue = ""
            do {
                let response = try await call(KaiRemoteRequest(action: "command", node: peer.node, device: peer.device,
                                                                command: command), as: KaiCommandResult.self)
                guard submitted == command else { return }
                if response.state == "uncertain" || response.state == "pending" {
                    issue = "The host has no final receipt yet. Check the chat before sending a new request."
                } else if let error = response.error ?? response.result?.error {
                    issue = error
                    if response.state == "rejected" { submitted = nil }
                } else {
                    draft = ""
                    submitted = nil
                    if chat == nil { await loadWorkspace() }
                }
            } catch { if submitted == command { issue = error.localizedDescription } }
        }
    }

    fileprivate func requestScreenshot(from peer: KaiRemotePeer) {
        Task {
            do {
                let value = try await call(KaiRemoteRequest(action: "request-screenshot", node: peer.node, device: peer.device),
                                           as: KaiRemoteScreenshot.self)
                if selected?.device == peer.device {
                    requestedScreenshot = value.id
                    issue = "Waiting for screenshot approval on the host Mac."
                }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func fetchScreenshot(from peer: KaiRemotePeer) {
        guard let id = requestedScreenshot else { return }
        Task {
            do {
                let raw = try await callData(KaiRemoteRequest(action: "fetch-screenshot", node: peer.node, device: peer.device,
                                                             screenshot: id))
                if selected?.device == peer.device {
                    screenshotImage = NSImage(data: raw)
                    requestedScreenshot = nil
                    issue = ""
                }
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func resolveScreenshot(_ request: KaiRemoteScreenshot, share: Bool) {
        Task {
            do {
                var file: URL?
                var scratch: URL?
                defer { if let scratch { try? FileManager.default.removeItem(at: scratch) } }
                if share {
                    guard #available(macOS 14, *) else {
                        issue = "Desktop screenshots require macOS 14 or newer."
                        return
                    }
                    guard CGPreflightScreenCaptureAccess() else {
                        _ = CGRequestScreenCaptureAccess()
                        issue = "Allow Screen Recording for Kai in System Settings, then approve the request again."
                        return
                    }
                    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                    guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() }) else {
                        issue = "Kai could not find the main display."
                        return
                    }
                    let filter = SCContentFilter(display: display, excludingWindows: [])
                    let config = SCStreamConfiguration()
                    let scale = min(1.0, 1600.0 / Double(max(display.width, display.height)))
                    config.width = max(1, Int(Double(display.width) * scale))
                    config.height = max(1, Int(Double(display.height) * scale))
                    config.showsCursor = true
                    let capture = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
                    guard let data = NSBitmapImageRep(cgImage: capture).representation(using: .png, properties: [:]) else {
                        issue = "Kai could not encode the screenshot."
                        return
                    }
                    let directory = FileManager.default.temporaryDirectory
                        .appendingPathComponent("kai-capture-\(UUID().uuidString)", isDirectory: true)
                    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false,
                                                            attributes: [.posixPermissions: 0o700])
                    scratch = directory
                    let url = directory.appendingPathComponent("screenshot.png")
                    try data.write(to: url, options: .withoutOverwriting)
                    file = url
                }
                _ = try await call(KaiRemoteRequest(action: "resolve-screenshot", screenshot: request.id, path: file?.path),
                                   as: KaiRemoteAck.self)
                await refreshStatus()
            } catch { issue = error.localizedDescription }
        }
    }

    fileprivate func checkRemoteUpdate(_ peer: KaiRemotePeer) {
        Task {
            do {
                var value = try await call(KaiRemoteRequest(action: "remote-update-status", node: peer.node,
                                                            device: peer.device), as: KaiRemoteUpdate.self)
                if value.state != "ready", peer.build ?? 0 >= 191 {
                    _ = try await call(KaiRemoteRequest(action: "remote-check-update", node: peer.node, device: peer.device),
                                       as: KaiRemoteAck.self)
                    for _ in 0..<8 {
                        try await Task.sleep(nanoseconds: 1_000_000_000)
                        value = try await call(KaiRemoteRequest(action: "remote-update-status", node: peer.node,
                                                               device: peer.device), as: KaiRemoteUpdate.self)
                        if value.state != "checking" { break }
                    }
                }
                if selected?.device == peer.device {
                    remoteUpdate = value
                    if let message = value.message { issue = message }
                }
            } catch { issue = error.localizedDescription }
        }
    }

    private func relaunch() throws {
        guard !relaunchStarted else { return }
        let app = Bundle.main.bundleURL
        let pid = ProcessInfo.processInfo.processIdentifier
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", "for n in $(seq 1 60); do if ! kill -0 \"$1\" 2>/dev/null; then exec /usr/bin/open \"$2\"; fi; sleep 1; done",
                             "kai-relaunch", String(pid), app.path]
        try process.run()
        quit?()
        relaunchStarted = true
    }

    private func showUpdateProblem(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Kai update"
        alert.informativeText = message
        alert.runModal()
    }

    func applyUpdate() {
        guard !restartRequested, let build = status?.update.build, status?.update.state == "ready" else { return }
        restartRequested = true
        Task {
            defer { restartRequested = false }
            do {
                _ = try await call(KaiRemoteRequest(action: "activate-update"), as: KaiRemoteAck.self)
                for _ in 0..<90 {
                    try await Task.sleep(nanoseconds: 1_000_000_000)
                    await refreshStatus()
                    if status?.update.state == "installed", status?.update.build == build {
                        try relaunch()
                        return
                    }
                    if status?.update.state == "error" {
                        throw NSError(domain: "KaiUpdate", code: 1, userInfo: [
                            NSLocalizedDescriptionKey: status?.update.message ?? "Kai update failed."
                        ])
                    }
                }
                showUpdateProblem("Kai did not confirm the update.")
            } catch { showUpdateProblem(error.localizedDescription) }
        }
    }
}

private struct KaiRemoteDevicesView: View {
    @ObservedObject var model: KaiRemoteDevices
    @State private var updateAllowed = false
    @State private var controlAllowed = false
    @State private var terminalAllowed = false
    @State private var provider = "codex"
    @State private var sharedProject = ""


    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Remote Devices").font(.headline)
                Spacer()
                if let status = model.status {
                    let usingBackup = status.systemTailnet == true && model.peers.contains { $0.online && $0.transport == "system" }
                    Text(usingBackup ? "Mac’s Tailscale connected" : status.network == "Running" ? "Tailnet connected" : status.network)
                        .foregroundStyle(.secondary)
                    if status.enabled {
                        Menu {
                            Button("Generate Pairing QR…") { model.generatePairingCode() }
                            Button("Paste Pairing Link…") { model.pastePairingLink() }
                            Divider()
                            if status.systemTailnet == true {
                                Button("Turn Off Tailscale Backup") { model.perform("disable-system-tailnet") }
                            } else {
                                Button("Use Mac’s Tailscale as Backup") { model.perform("enable-system-tailnet") }
                            }
                            Button("Turn Off Remote Access") { model.perform("disable") }
                        } label: {
                            Image(systemName: "ellipsis.circle")
                        }.menuStyle(.borderlessButton).fixedSize()
                    }
                }
                Menu("Updates") {
                    Button("Check for Updates") { model.perform("check-update") }
                    Button("Prepare Signed ZIP…") { model.selectUpdateArchive() }
                }.fixedSize()
                if model.status?.update.state == "ready" {
                    Button("Restart to Update") { model.applyUpdate() }.buttonStyle(.borderedProminent)
                }
            }.padding(12)
            if let status = model.status {
                if let connectionIssue = status.error {
                    Text(connectionIssue)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12).padding(.bottom, 10)
                }
                if status.systemTailnet == true && model.selected?.transport == "system" && model.selected?.allowed == false {
                    Text("The host needs to approve this Mac’s Tailscale app connection.")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12).padding(.bottom, 10)
                }
                if !status.enabled {
                    HStack {
                        Text("Connect this Mac to your tailnet to find other Kai devices.")
                        Spacer()
                        Button("Enable Remote Access") { model.perform("enable") }
                    }.padding(.horizontal, 12).padding(.bottom, 10)
                }
                if let auth = status.authUrl {
                    HStack {
                        Text("Sign in to Tailscale to finish enrolling this Mac.")
                        Spacer()
                        Button("Sign in to Tailscale") { NSWorkspace.shared.open(auth) }
                    }.padding(.horizontal, 12).padding(.bottom, 10)
                }
                if let capture = status.screenshots.first {
                    HStack {
                        Text("\(capture.peer.name) requests a screenshot of your main display.")
                        Spacer()
                        Button("Decline") { model.resolveScreenshot(capture, share: false) }
                        Button("Share Screenshot") { model.resolveScreenshot(capture, share: true) }
                            .buttonStyle(.borderedProminent)
                    }.padding(12).background(.quaternary)
                }
                if let request = status.requests.first {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("\(request.peer.name) is requesting access").fontWeight(.semibold)
                        Text("Allow this device to read your Kai project and chat history.").foregroundStyle(.secondary)
                        if let account = request.peer.account { Text(account) }
                        Toggle("Allow this device to send agent tasks", isOn: $controlAllowed)
                        Picker("Share project files", selection: $sharedProject) {
                            Text("None").tag("")
                            ForEach(model.localProjects) { project in Text(project.name).tag(project.path) }
                        }
                        if !sharedProject.isEmpty && controlAllowed {
                            Toggle("Allow interactive terminals", isOn: $terminalAllowed)
                            Text(terminalAllowed ? "A terminal can run commands as your macOS user. Shared text files are also editable." :
                                    "This device can edit text files in the shared project.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        HStack {
                            Toggle("Allow updates from this device", isOn: $updateAllowed)
                            Spacer()
                            Button("Deny") { model.perform("deny", peer: request.peer); updateAllowed = false; controlAllowed = false }
                            Button("Allow") { model.perform("allow", peer: request.peer, update: updateAllowed, control: controlAllowed,
                                               terminal: terminalAllowed, project: sharedProject.isEmpty ? nil : sharedProject)
                                updateAllowed = false
                                controlAllowed = false
                                terminalAllowed = false
                                sharedProject = "" }
                                .buttonStyle(.borderedProminent)
                        }
                    }.padding(12).background(.quaternary)
                     .task(id: request.id) {
                        sharedProject = ""
                        terminalAllowed = false
                        await model.loadLocalProjects()
                     }
                }
            }
            Divider()
            HSplitView {
                List {
                    Section("On this tailnet") {
                        ForEach(model.peers) { peer in
                            Button { model.select(peer) } label: {
                                Label(peer.name, systemImage: peer.allowed ? "desktopcomputer" : "network")
                                    .foregroundStyle(model.selected?.id == peer.id ? Color.accentColor : Color.primary)
                            }.buttonStyle(.plain)
                        }
                    }
                    if let status = model.status, !status.trusted.isEmpty {
                        Section("Allowed on this Mac") {
                            ForEach(status.trusted) { peer in
                                HStack {
                                    Text(peer.name)
                                    Spacer()
                                    Button("Revoke") { model.perform("revoke", peer: peer) }
                                }
                            }
                        }
                    }
                }.frame(minWidth: 195, idealWidth: 230)
                detail.frame(minWidth: 340, maxWidth: .infinity, maxHeight: .infinity)
            }
            Divider()
            HStack {
                Text(model.issue.isEmpty ? model.status?.update.message ?? "" : model.issue)
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                Spacer()
                if let status = model.status { Text(status.device).font(.caption.monospaced()).textSelection(.enabled) }
                Button("Refresh") { Task { await model.discover(); await model.loadWorkspace() } }
            }.padding(8)
        }
        .sheet(isPresented: $model.showRunControls) {
            KaiRemoteRunControlSheet(model: model)
        }
        .sheet(item: $model.pairingCode) { code in
            VStack(spacing: 14) {
                Text("Pair a Kai Device").font(.headline)
                Text("This one-time link grants read access on your tailnet. It expires in five minutes.")
                    .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                if let qr = code.image() {
                    Image(nsImage: qr).interpolation(.none).resizable()
                        .frame(width: 180, height: 180).padding(8).background(.white)
                        .accessibilityLabel("Kai pairing QR code")
                }
                Button("Copy Pairing Link") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(code.link, forType: .string)
                }
                Button("Done") { model.pairingCode = nil }.keyboardShortcut(.defaultAction)
            }.padding(22).frame(width: 380)
        }
    }

    @ViewBuilder private func terminalBrowser(_ peer: KaiRemotePeer) -> some View {
        HStack {
            Menu(model.attachedTerminal?.program.capitalized ?? "Sessions") {
                ForEach(model.terminals) { terminal in
                    Button(terminal.program.capitalized + " · " + String(terminal.id.prefix(6))) {
                        model.attachedTerminal = terminal
                    }
                }
            }
            Spacer()
            Menu("New Terminal") {
                Button("Shell") { model.startTerminal(program: "shell") }
                Button("Codex CLI") { model.startTerminal(program: "codex") }
                Button("Claude CLI") { model.startTerminal(program: "claude") }
            }.disabled(model.busy || !peer.online)
        }
        if let terminal = model.attachedTerminal {
            KaiRemoteTerminalView(arguments: model.terminalArguments(peer, terminal: terminal))
                .id(terminal.id)
                .clipShape(RoundedRectangle(cornerRadius: 7))
        } else {
            Text("Open a terminal or start a new session.").foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    @ViewBuilder private func fileBrowser(_ directory: String) -> some View {
        HStack {
            if directory != "." {
                Button("Parent Folder") {
                    let parent = (directory as NSString).deletingLastPathComponent
                    model.browseFiles(parent.isEmpty ? "." : parent)
                }
            }
            Text(directory == "." ? model.selected?.fileProject.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "Project Files" : directory)
                .font(.subheadline.weight(.semibold)).lineLimit(1)
            Spacer()
            if model.selected?.build ?? 0 >= 195 {
                Button(model.artifactPanel ? "Files" : "Artifacts") {
                    if model.artifactPanel { model.artifactPanel = false } else { model.loadArtifacts() }
                }
                if model.selected?.control == true && !model.artifactPanel {
                    Button("Upload…") { model.uploadFile() }.disabled(model.busy)
                }
            }
            Button("Reload Files") { model.browseFiles(directory) }
        }
        if model.artifactPanel {
            artifactBrowser
        } else {
        if model.selected?.build ?? 0 >= 195 {
            TextField("Find a file in this folder", text: $model.fileSearch)
                .onSubmit { model.searchProjectFiles() }
        }
        HSplitView {
            List {
                ForEach(model.searchResults ?? model.directory?.entries ?? []) { entry in
                    Button { model.openFile(entry, exact: model.searchResults != nil) } label: {
                        Label(entry.name, systemImage: entry.kind == "folder" ? "folder" : "doc").lineLimit(1)
                    }.buttonStyle(.plain)
                }
                if model.directory?.next != nil {
                    Button("More Files") { model.browseFiles(directory, more: true) }
                }
            }.frame(minWidth: 145, idealWidth: 195)
            VStack(alignment: .leading, spacing: 8) {
                if let path = model.filePath {
                    Text(URL(fileURLWithPath: path).lastPathComponent).font(.headline).lineLimit(1)
                    if let file = model.filePreview {
                        HStack {
                            if file.editable {
                                Button("Save Edit") { model.saveFileEdit() }.disabled(model.busy || file.text == model.fileDraft)
                            }
                            if file.text == nil { Button("Open Preview") { model.downloadFile(preview: true) }.disabled(model.busy) }
                            Button("Download…") { model.downloadFile(preview: false) }.disabled(model.busy)
                            if model.selected?.build ?? 0 >= 195 {
                                Button(model.fileDiff == nil ? "Show Diff" : "Close Diff") { model.loadFileDiff() }
                            }
                        }
                        if let diff = model.fileDiff {
                            ScrollView {
                                Text(diff.isEmpty ? "No Git changes in this file." : diff)
                                    .font(.system(.body, design: .monospaced)).textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        } else if let url = model.richPreview {
                            KaiRemoteQuickLook(url: url)
                        } else if file.editable {
                            TextEditor(text: $model.fileDraft).font(.system(.body, design: .monospaced))
                        } else if let text = file.text {
                            ScrollView {
                                Text(text).font(.system(.body, design: .monospaced)).textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                    }
                } else {
                    Text("Select a file").foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }.padding(8).frame(minWidth: 175, maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        }
    }

    private var artifactBrowser: some View {
        HSplitView {
            List {
                ForEach(model.artifacts?.artifacts ?? []) { item in
                    Button(item.title) { model.loadArtifact(item) }
                }
                if model.artifacts?.nextOffset != nil {
                    Button("More Artifacts") { model.loadArtifacts(more: true) }
                }
            }.frame(minWidth: 145, idealWidth: 195)
            VStack(alignment: .leading) {
                if let item = model.artifact {
                    HStack {
                        Text(item.title).font(.headline)
                        Spacer()
                        Button("Save Copy…") { model.saveArtifactCopy() }
                    }
                    ScrollView {
                        if let content = item.content {
                            if item.kind == "markdown", let styled = try? AttributedString(markdown: content,
                                    options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)) {
                                Text(styled).textSelection(.enabled)
                            } else { Text(content).font(.system(.body, design: .monospaced)).textSelection(.enabled) }
                        }
                    }
                } else {
                    Text("Select an artifact").foregroundStyle(.secondary)
                }
            }.padding(8).frame(minWidth: 175, maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
    }

    @ViewBuilder private var detail: some View {
        if let peer = model.selected {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(peer.name).font(.title3.weight(.semibold))
                    Spacer()
                    if peer.allowed {
                        if peer.terminal == true, peer.fileProject != nil,
                           peer.capabilities?.contains("workspace.terminal") == true {
                            Button(model.terminalsOpen ? "Chats" : "Terminal") {
                                if model.terminalsOpen { model.closeTerminals(); model.returnToChats() }
                                else { model.showTerminals() }
                            }
                        }
                        if peer.fileProject != nil {
                            Button(model.directoryPath == nil ? "Project Files" : "Chats") {
                                if model.directoryPath == nil { model.closeTerminals(); model.browseFiles() }
                                else { model.closeFiles(); model.returnToChats() }
                            }
                        }
                        Button("Disconnect") { model.perform("disconnect", peer: peer) }
                        if peer.capabilities?.contains("workspace.screenshot") == true {
                            if model.requestedScreenshot != nil {
                                Button("Get Screenshot") { model.fetchScreenshot(from: peer) }
                            } else {
                                Button("Request Screenshot") { model.requestScreenshot(from: peer) }
                            }
                        }
                        if peer.update == true {
                            if model.remoteUpdate?.state == "ready" {
                                Button("Update & Relaunch") { model.perform("remote-update", peer: peer) }
                            } else {
                                Button("Check Remote Update") { model.checkRemoteUpdate(peer) }
                            }
                        }
                    } else if peer.online && peer.compatible {
                        Button("Connect") { model.perform("pair", peer: peer) }.disabled(model.busy)
                    }
                }
                if !peer.online {
                    Text("Offline. The Mac must be awake and connected.").foregroundStyle(.secondary)
                } else if !peer.compatible {
                    Text("Update Kai on one or both Macs. Their remote protocols do not overlap.").foregroundStyle(.secondary)
                } else if !peer.allowed {
                    Text("Approve the connection in Remote Devices on \(peer.name).").foregroundStyle(.secondary)
                }
                if let image = model.screenshotImage {
                    Button("Back to Workspace") { model.screenshotImage = nil }
                    Image(nsImage: image).resizable().scaledToFit()
                } else if model.terminalsOpen {
                    terminalBrowser(peer)
                } else if let path = model.directoryPath {
                    fileBrowser(path)
                } else if let workspace = model.workspace {
                    HStack {
                        Menu(model.project.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "All Projects") {
                            Button("All Projects") { model.selectProject(nil) }
                            ForEach(workspace.projects) { project in Button(project.name) { model.selectProject(project.path) } }
                        }
                        if peer.control == true, model.project != nil {
                            Button("New Project…") { model.createProject() }.disabled(model.busy)
                        }
                        if model.selectedChat != nil {
                            Button("All Chats") {
                                model.returnToChats()
                            }
                        }
                    }
                    if let chat = model.selectedChat {
                        HStack {
                            Text(chat.title).font(.headline)
                            Spacer()
                            if peer.control == true, peer.build ?? 0 >= 195 {
                                Button("Run Controls…") { model.openRunControls() }
                            }
                        }
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 15) {
                                if model.messages.count >= 30 { Button("Load Earlier") { model.loadEarlier() } }
                                ForEach(model.messages) { message in
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(message.role).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                                        Text(message.text).textSelection(.enabled)
                                        if message.truncated { Text("Preview truncated").font(.caption).foregroundStyle(.secondary) }
                                    }.frame(maxWidth: .infinity, alignment: .leading)
                                }
                                ForEach(Array(model.liveTools.enumerated()), id: \.offset) { _, tool in
                                    Label(tool, systemImage: "wrench.and.screwdriver").font(.caption).foregroundStyle(.secondary)
                                }
                                if let live = model.live {
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(live.status).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                                        if let tail = live.tail, !tail.isEmpty { Text(tail).textSelection(.enabled) }
                                    }.frame(maxWidth: .infinity, alignment: .leading)
                                }
                            }.padding(5)
                        }
                        if peer.control == true && peer.online {
                            HStack {
                                TextField("Send a message to this chat", text: $model.draft).onSubmit { model.sendDraft() }
                                Button("Send") { model.sendDraft() }.disabled(model.busy || model.draft.isEmpty)
                            }
                        }
                    } else {
                        List {
                            ForEach(workspace.chats) { chat in
                                Button { model.selectChat(chat) } label: {
                                    VStack(alignment: .leading) {
                                        Text(chat.title).lineLimit(1)
                                        let status = model.activity.first(where: { $0.chat == chat.id })?.status
                                        Text([chat.agent, status].compactMap { $0 }.joined(separator: " · "))
                                            .font(.caption).foregroundStyle(.secondary)
                                    }
                                }.buttonStyle(.plain)
                            }
                            if workspace.nextOffset != nil {
                                Button("More Chats") { Task { await model.loadWorkspace(more: true) } }
                            }
                        }
                        if peer.control == true && peer.online && model.project != nil {
                            HStack {
                                Picker("Agent", selection: $provider) {
                                    Text("Codex").tag("codex")
                                    Text("Claude").tag("claude")
                                }.fixedSize()
                                TextField("Start a new chat in this project", text: $model.draft)
                                    .onSubmit { model.sendDraft(agent: provider) }
                                Button("Send") { model.sendDraft(agent: provider) }.disabled(model.busy || model.draft.isEmpty)
                            }
                        }
                    }
                }
                Spacer(minLength: 0)
            }.padding(12)
        } else {
            Text("Select a Mac").foregroundStyle(.secondary)
        }
    }
}

private struct KaiRemoteQuickLook: NSViewRepresentable {
    let url: URL
    func makeNSView(context: Context) -> QLPreviewView {
        QLPreviewView(frame: .zero, style: .normal)
    }
    func updateNSView(_ view: QLPreviewView, context: Context) {
        view.previewItem = url as NSURL
    }
}

private struct KaiRemoteRunControlSheet: View {
    @ObservedObject var model: KaiRemoteDevices
    @State private var steer = ""
    @State private var answers: [String: [String]] = [:]

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Run Controls").font(.headline)
                Spacer()
                Button("Refresh") { model.refreshRunControls() }
                Button("Done") { model.showRunControls = false }
            }
            if let active = model.runControls?.tasks.first, let id = active.runId {
                HStack {
                    Text(active.status)
                    Spacer()
                    Button("Stop Task", role: .destructive) { model.sendRunControl("stop", runID: id) }
                }
                HStack {
                    TextField("Steer a supported running agent", text: $steer)
                    Button("Send") {
                        model.sendRunControl("steer", runID: id, prompt: steer)
                        steer = ""
                    }.disabled(steer.isEmpty || model.busy)
                }
            }
            if let approval = model.runControls?.approvals.first {
                ScrollView {
                    VStack(alignment: .leading, spacing: 9) {
                        Text(approval.request.title).font(.subheadline.bold())
                        Text(approval.request.detail).textSelection(.enabled)
                        if let choices = approval.request.requestKind.approval?.choices {
                            ForEach(choices) { choice in
                                Button(choice.label) {
                                    model.sendRunControl("answer", answer: KaiRemoteDecision(approvalRequestId: approval.id, choice: choice.id))
                                }.disabled(model.busy)
                            }
                        } else if let questions = approval.request.requestKind.questions?.questions {
                            ForEach(questions) { question in
                                Text(question.question)
                                if question.options.isEmpty {
                                    let value = Binding(get: { answers[question.id]?.first ?? "" },
                                                        set: { answers[question.id] = [$0] })
                                    if question.secret { SecureField("Answer", text: value) }
                                    else { TextField("Answer", text: value) }
                                } else if question.multiple {
                                    ForEach(question.options) { option in
                                        Toggle(option.label, isOn: Binding(
                                            get: { answers[question.id]?.contains(option.id) == true },
                                            set: { enabled in
                                                var values = answers[question.id] ?? []
                                                values.removeAll { $0 == option.id }
                                                if enabled { values.append(option.id) }
                                                answers[question.id] = values
                                            }))
                                    }
                                } else {
                                    Picker("Answer", selection: Binding(
                                        get: { answers[question.id]?.first ?? "" },
                                        set: { answers[question.id] = [$0] })) {
                                        Text("Choose…").tag("")
                                        ForEach(question.options) { option in Text(option.label).tag(option.id) }
                                    }
                                }
                            }
                            Button("Submit Answers") {
                                model.sendRunControl("answer", answer: KaiRemoteDecision(approvalRequestId: approval.id, answers: answers))
                                answers = [:]
                            }.disabled(model.busy || questions.contains { $0.required && (answers[$0.id]?.allSatisfy { $0.isEmpty } ?? true) })
                        }
                    }
                }.frame(maxHeight: 260)
            } else if model.runControls?.tasks.first?.runId == nil {
                Text("No active task or pending approval.").foregroundStyle(.secondary)
            }
            if !model.issue.isEmpty { Text(model.issue).font(.caption).foregroundStyle(.secondary) }
        }.padding(20).frame(width: 490)
    }
}
