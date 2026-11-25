import Foundation
import QuartzCore

struct NinjaActorFrameRequest: @unchecked Sendable {
    let keys: [NinjaPixelActor.Key]
    let palette: NinjaActorRaster.Palette
    let diagnostics: Bool

    init(keys: [NinjaPixelActor.Key], palette: NinjaActorRaster.Palette,
         diagnostics: Bool = false) {
        var seen = Set<NinjaPixelActor.Key>()
        self.keys = keys.filter { seen.insert($0).inserted }
        self.palette = palette
        self.diagnostics = diagnostics
    }
}

struct NinjaActorFrameResult: @unchecked Sendable {
    let frames: [NinjaPixelActor.Key: CGImage]
    let generated: Int
    let cacheBytes: Int
    let rasterMicroseconds: Int64?
}

/// Instances have one owner. Production uses the shared serial raster worker.
final class NinjaActorRasterStore: @unchecked Sendable {
    private let countLimit: Int
    private let byteLimit: Int
    private var cache: [NinjaPixelActor.Key: CGImage] = [:]
    private var order: [NinjaPixelActor.Key] = []
    private(set) var bytes = 0
    var count: Int { cache.count }

    init(countLimit: Int = 256, byteLimit: Int = 24 * 1024 * 1024) {
        self.countLimit = countLimit
        self.byteLimit = byteLimit
    }

    func render(_ request: NinjaActorFrameRequest) -> NinjaActorFrameResult? {
        let began = request.diagnostics ? CACurrentMediaTime() : nil
        var frames: [NinjaPixelActor.Key: CGImage] = [:]
        var generated = 0
        for key in request.keys {
            if let image = cache[key] {
                touch(key)
                frames[key] = image
                continue
            }
            guard let image = NinjaActorRaster.frame(key, palette: request.palette) else { return nil }
            generated += 1
            frames[key] = image
            let cost = image.bytesPerRow * image.height
            if countLimit > 0, cost <= byteLimit {
                while !order.isEmpty && (cache.count >= countLimit || bytes + cost > byteLimit) {
                    let oldest = order.removeFirst()
                    if let old = cache.removeValue(forKey: oldest) { bytes -= old.bytesPerRow * old.height }
                }
                cache[key] = image
                order.append(key)
                bytes += cost
            }
        }
        let elapsed = began.map { Int64(((CACurrentMediaTime() - $0) * 1_000_000).rounded()) }
        return NinjaActorFrameResult(frames: frames, generated: generated,
                                     cacheBytes: bytes, rasterMicroseconds: elapsed)
    }

    private func touch(_ key: NinjaPixelActor.Key) {
        order.removeAll { $0 == key }
        order.append(key)
    }
}

/// One active worker globally, one queued entry and latest pending request per live view.
/// Matching frame keys reuse immutable CGImages across views through the shared LRU.
@MainActor final class NinjaActorFramePipeline {
    typealias Executor = (NinjaActorFrameRequest, @escaping (NinjaActorFrameResult?) -> Void) -> Void

    private final class Waiting {
        weak var pipeline: NinjaActorFramePipeline?
        init(_ pipeline: NinjaActorFramePipeline) { self.pipeline = pipeline }
    }
    private static let worker = DispatchQueue(label: "kai.ninja-actor-raster", qos: .userInitiated)
    private static let store = NinjaActorRasterStore()
    private static var waiting: [Waiting] = []
    private static var workerBusy = false

    private let execute: Executor
    private var active: NinjaActorFrameRequest?
    private var pending: NinjaActorFrameRequest?
    private var current: NinjaActorFrameRequest?
    private var published: NinjaActorFrameResult?
    private var completion: ((NinjaActorFrameResult?) -> Void)?
    private var revision: UInt64 = 0
    private var queued = false
    var activeCount: Int { active == nil ? 0 : 1 }
    var pendingCount: Int { pending == nil ? 0 : 1 }
    static var diagnosticActiveWorkerCount: Int { workerBusy ? 1 : 0 }
    static var diagnosticQueuedViewCount: Int { waiting.filter { $0.pipeline?.pending != nil }.count }

    init(execute: Executor? = nil) {
        self.execute = execute ?? { request, done in
            let store = Self.store
            Self.worker.async {
                assert(!Thread.isMainThread)
                let result = autoreleasepool { store.render(request) }
                DispatchQueue.main.async { done(result) }
            }
        }
    }

    func submit(_ request: NinjaActorFrameRequest,
                completion: @escaping (NinjaActorFrameResult?) -> Void) {
        if current?.keys == request.keys {
            self.completion = completion
            if let published { completion(published) }
            return
        }
        revision &+= 1
        current = request
        published = nil
        self.completion = completion
        pending = request
        enqueue()
    }

    func cancel() {
        revision &+= 1
        current = nil
        pending = nil
        published = nil
        completion = nil
        Self.waiting.removeAll { $0.pipeline === self }
        queued = false
    }

    private func enqueue() {
        if !queued {
            Self.waiting.removeAll { $0.pipeline == nil }
            queued = true
            Self.waiting.append(Waiting(self))
        }
        Self.pump()
    }

    private static func pump() {
        guard !workerBusy else { return }
        while !waiting.isEmpty {
            guard let pipeline = waiting.removeFirst().pipeline else { continue }
            pipeline.queued = false
            guard let request = pipeline.pending else { continue }
            pipeline.pending = nil
            pipeline.active = request
            let token = pipeline.revision
            workerBusy = true
            pipeline.execute(request) { [weak pipeline] result in
                MainActor.assumeIsolated {
                    pipeline?.finish(request, token: token, result: result)
                    Self.workerBusy = false
                    Self.pump()
                }
            }
            return
        }
    }

    private func finish(_ request: NinjaActorFrameRequest, token: UInt64,
                        result: NinjaActorFrameResult?) {
        active = nil
        if token == revision, current?.keys == request.keys {
            published = result
            if case nil = result { current = nil }
            completion?(result)
        }
    }
}
