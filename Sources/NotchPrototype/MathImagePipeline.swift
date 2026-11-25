import CoreGraphics
import CoreImage
import Foundation
import Metal

/// Only immutable pixel data and value parameters cross the worker boundary.
struct MathImageRequest: @unchecked Sendable {
    let image: CGImage
    let startColor: String
    let endColor: String
}

struct MathImageResult: @unchecked Sendable {
    let image: CGImage
    /// This records the selected context, independently of GPU timing evidence.
    let metalContextSelected: Bool
}

/// The shared serial queue exclusively owns this context and its filter jobs.
final class MathImageRasterStore: @unchecked Sendable {
    private var context: CIContext?
    private var metalContextSelected = false

    func render(_ request: MathImageRequest) -> MathImageResult? {
        assert(!Thread.isMainThread)
        guard let start = UInt64(request.startColor.dropFirst(), radix: 16),
              let end = UInt64(request.endColor.dropFirst(), radix: 16),
              let filter = CIFilter(name: "CIColorMatrix") else { return nil }
        let input = CIImage(cgImage: request.image)
        func color(_ value: UInt64) -> CIColor {
            CIColor(red: CGFloat((value >> 16) & 255) / 255,
                    green: CGFloat((value >> 8) & 255) / 255,
                    blue: CGFloat(value & 255) / 255)
        }
        let zero = CIVector(x: 0, y: 0, z: 0, w: 0)
        filter.setValue(input, forKey: kCIInputImageKey)
        filter.setValue(zero, forKey: "inputRVector")
        filter.setValue(zero, forKey: "inputGVector")
        filter.setValue(zero, forKey: "inputBVector")
        filter.setValue(CIVector(x: 1, y: 0, z: 0, w: 0), forKey: "inputAVector")
        filter.setValue(CIVector(x: 1, y: 1, z: 1, w: 0), forKey: "inputBiasVector")
        guard let mask = filter.outputImage,
              let gradient = CIFilter(name: "CILinearGradient", parameters: [
                "inputPoint0": CIVector(x: input.extent.minX, y: input.extent.midY),
                "inputPoint1": CIVector(x: input.extent.maxX, y: input.extent.midY),
                "inputColor0": color(start), "inputColor1": color(end)
              ])?.outputImage,
              let output = CIFilter(name: "CISourceInCompositing", parameters: [
                kCIInputImageKey: gradient, kCIInputBackgroundImageKey: mask
              ])?.outputImage else { return nil }
        if context == nil {
            let options: [CIContextOption: Any] = [.cacheIntermediates: false]
            if let device = MTLCreateSystemDefaultDevice() {
                context = CIContext(mtlDevice: device, options: options)
                metalContextSelected = true
            } else {
                context = CIContext(options: options)
            }
        }
        guard let image = context?.createCGImage(output, from: input.extent) else { return nil }
        return MathImageResult(image: image, metalContextSelected: metalContextSelected)
    }
}

/// One active raster globally, and one latest pending request per live renderer.
/// Scheduling follows the existing NinjaActorFramePipeline ownership pattern.
@MainActor final class MathImagePipeline {
    typealias Completion = (MathImageResult?) -> Void
    typealias Executor = (MathImageRequest, @escaping Completion) -> Void
    private struct Pending {
        let request: MathImageRequest
        let completion: Completion
        let revision: UInt64
    }
    private final class Waiting {
        weak var pipeline: MathImagePipeline?
        init(_ pipeline: MathImagePipeline) { self.pipeline = pipeline }
    }
    private static let worker = DispatchQueue(label: "kai.math-image-raster", qos: .userInitiated)
    private static let store = MathImageRasterStore()
    private static var waiting: [Waiting] = []
    private static var workerBusy = false
    private let execute: Executor
    private var pending: Pending?
    private var revision: UInt64 = 0

    #if DEBUG
    var pendingCount: Int { pending == nil ? 0 : 1 }
    static var diagnosticActiveWorkerCount: Int { workerBusy ? 1 : 0 }
    static var diagnosticQueuedRendererCount: Int {
        waiting.filter { $0.pipeline?.pending != nil }.count
    }
    #endif

    init(execute: Executor? = nil) {
        self.execute = execute ?? { request, completion in
            let store = Self.store
            Self.worker.async {
                let result = autoreleasepool { store.render(request) }
                DispatchQueue.main.async { completion(result) }
            }
        }
    }

    func submit(_ request: MathImageRequest, completion: @escaping Completion) {
        revision &+= 1
        pending = Pending(request: request, completion: completion, revision: revision)
        Self.waiting.removeAll { $0.pipeline == nil }
        if !Self.waiting.contains(where: { $0.pipeline === self }) {
            Self.waiting.append(Waiting(self))
        }
        Self.pump()
    }

    func cancel() {
        revision &+= 1
        pending = nil
        Self.waiting.removeAll { $0.pipeline === self }
    }

    private static func pump() {
        guard !workerBusy else { return }
        while !waiting.isEmpty {
            guard let pipeline = waiting.removeFirst().pipeline,
                  let job = pipeline.pending else { continue }
            pipeline.pending = nil
            workerBusy = true
            pipeline.execute(job.request) { [weak pipeline] result in
                MainActor.assumeIsolated {
                    if let pipeline, pipeline.revision == job.revision {
                        job.completion(result)
                    }
                    Self.workerBusy = false
                    Self.pump()
                }
            }
            return
        }
    }
}
