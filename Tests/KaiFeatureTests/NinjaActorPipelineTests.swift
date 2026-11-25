import AppKit
import XCTest
@testable import NotchPrototype

final class NinjaActorPipelineTests: XCTestCase {
    @MainActor final class Jobs {
        var started: [NinjaActorFrameRequest] = []
        var pending: [(NinjaActorFrameRequest, (NinjaActorFrameResult?) -> Void)] = []
        func execute(_ request: NinjaActorFrameRequest, done: @escaping (NinjaActorFrameResult?) -> Void) {
            started.append(request)
            pending.append((request, done))
        }
        @discardableResult func finish(success: Bool = true) -> NinjaActorFrameResult? {
            let (request, done) = pending.removeFirst()
            var frames: [NinjaPixelActor.Key: CGImage] = [:]
            for (index, key) in request.keys.enumerated() {
                let context = CGContext(data: nil, width: 2, height: 2, bitsPerComponent: 8,
                    bytesPerRow: 8, space: CGColorSpaceCreateDeviceRGB(),
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
                context.setFillColor(CGColor(red: CGFloat(index % 13) / 13,
                    green: CGFloat(index % 7) / 7, blue: 0.5, alpha: 1))
                context.fill(CGRect(x: 0, y: 0, width: 2, height: 2))
                frames[key] = context.makeImage()!
            }
            let result = success ? NinjaActorFrameResult(frames: frames,
                generated: request.keys.count, cacheBytes: frames.values.reduce(0) { $0 + $1.bytesPerRow * $1.height },
                rasterMicroseconds: nil) : nil
            done(result)
            return result
        }
        func drain() { while !pending.isEmpty { finish() } }
    }

    @MainActor func request(_ style: String = "ice", step: Int = 0) -> NinjaActorFrameRequest {
        NinjaActorFrameRequest(keys: [.init(style: style, action: .rest, step: step)],
                               palette: NinjaPixelActor.palette(style: style))
    }

    @MainActor func testIdenticalRequestsCoalesceAndUseLatestCallback() {
        let jobs = Jobs(); defer { jobs.drain() }
        let pipeline = NinjaActorFramePipeline(execute: jobs.execute)
        var old = 0, latest = 0, cached = 0
        pipeline.submit(request()) { _ in old += 1 }
        pipeline.submit(request()) { _ in latest += 1 }
        XCTAssertEqual(jobs.started.count, 1)
        jobs.finish()
        XCTAssertEqual(old, 0); XCTAssertEqual(latest, 1)
        pipeline.submit(request()) { _ in cached += 1 }
        XCTAssertEqual(cached, 1); XCTAssertEqual(jobs.started.count, 1)
    }

    @MainActor func testOnlyLatestPendingRequestRendersAndPublishes() {
        let jobs = Jobs(); defer { jobs.drain() }
        let pipeline = NinjaActorFramePipeline(execute: jobs.execute)
        var accepted: [String] = []
        for style in ["ice", "fire", "earth"] {
            pipeline.submit(request(style)) { _ in accepted.append(style) }
        }
        XCTAssertEqual(pipeline.activeCount, 1); XCTAssertEqual(pipeline.pendingCount, 1)
        XCTAssertEqual(jobs.started.count, 1)
        jobs.finish()
        XCTAssertTrue(accepted.isEmpty)
        XCTAssertEqual(jobs.started.count, 2)
        XCTAssertEqual(jobs.started[1].keys[0].style, "earth")
        jobs.finish()
        XCTAssertEqual(accepted, ["earth"])
    }

    @MainActor func testABASupersedesOriginalAResult() {
        let jobs = Jobs(); defer { jobs.drain() }
        let pipeline = NinjaActorFramePipeline(execute: jobs.execute)
        var accepted = 0
        pipeline.submit(request("ice")) { _ in XCTFail("Original A is stale") }
        pipeline.submit(request("fire")) { _ in XCTFail("B was replaced") }
        pipeline.submit(request("ice")) { _ in accepted += 1 }
        jobs.finish()
        XCTAssertEqual(accepted, 0)
        XCTAssertEqual(jobs.started.count, 2)
        jobs.finish()
        XCTAssertEqual(accepted, 1)
    }

    @MainActor func testCancelAndResubmitSameKeysRejectsOldGeneration() {
        let jobs = Jobs(); defer { jobs.drain() }
        let pipeline = NinjaActorFramePipeline(execute: jobs.execute)
        var accepted = 0
        pipeline.submit(request()) { _ in XCTFail("Canceled work published") }
        pipeline.cancel()
        pipeline.submit(request()) { _ in accepted += 1 }
        jobs.finish()
        XCTAssertEqual(accepted, 0)
        jobs.finish()
        XCTAssertEqual(accepted, 1)
    }

    @MainActor func testGlobalWorkerHasOneActiveAndOneLatestEntryPerView() {
        let jobs = Jobs(); defer { jobs.drain() }
        let first = NinjaActorFramePipeline(execute: jobs.execute)
        let second = NinjaActorFramePipeline(execute: jobs.execute)
        first.submit(request()) { _ in }
        second.submit(request("fire")) { _ in XCTFail("Replaced pending view request") }
        second.submit(request("earth")) { _ in }
        XCTAssertEqual(NinjaActorFramePipeline.diagnosticActiveWorkerCount, 1)
        XCTAssertEqual(NinjaActorFramePipeline.diagnosticQueuedViewCount, 1)
        XCTAssertEqual(jobs.started.count, 1)
        jobs.finish()
        XCTAssertEqual(jobs.started.count, 2)
        XCTAssertEqual(jobs.started[1].keys[0].style, "earth")
        jobs.finish()
        XCTAssertEqual(NinjaActorFramePipeline.diagnosticActiveWorkerCount, 0)
    }

    @MainActor func testCanceledOrDeallocatedWaitingViewsNeverStart() {
        let jobs = Jobs(); defer { jobs.drain() }
        let first = NinjaActorFramePipeline(execute: jobs.execute)
        var waiting: NinjaActorFramePipeline? = NinjaActorFramePipeline(execute: jobs.execute)
        first.submit(request()) { _ in }
        waiting!.submit(request("fire")) { _ in XCTFail("Canceled waiting view published") }
        waiting!.cancel()
        waiting!.submit(request("earth")) { _ in XCTFail("Deallocated view published") }
        weak var weakWaiting = waiting
        waiting = nil
        XCTAssertNil(weakWaiting)
        jobs.finish()
        XCTAssertEqual(jobs.started.count, 1)
        XCTAssertEqual(NinjaActorFramePipeline.diagnosticActiveWorkerCount, 0)
    }

    @MainActor func testCompletionCanSynchronouslySubmitNextRequest() {
        let jobs = Jobs(); defer { jobs.drain() }
        let pipeline = NinjaActorFramePipeline(execute: jobs.execute)
        var accepted: [String] = []
        pipeline.submit(request()) { _ in
            accepted.append("ice")
            pipeline.submit(self.request("fire")) { _ in accepted.append("fire") }
        }
        jobs.finish()
        XCTAssertEqual(jobs.started.count, 2)
        jobs.finish()
        XCTAssertEqual(accepted, ["ice", "fire"])
    }

    @MainActor func testFailedRenderCanRetryIdenticalKeys() {
        let jobs = Jobs(); defer { jobs.drain() }
        let pipeline = NinjaActorFramePipeline(execute: jobs.execute)
        pipeline.submit(request()) { XCTAssertNil($0) }
        jobs.finish(success: false)
        pipeline.submit(request()) { XCTAssertNotNil($0) }
        XCTAssertEqual(jobs.started.count, 2)
        jobs.finish()
    }

    @MainActor func testKeyNormalizationPreservesNegativeSwiftRemainders() {
        for step in [-17, -9, -8, -7, -1, 0, 7, 8, 9, 17] {
            let key = NinjaPixelActor.Key(style: "ice", action: .wave, step: step)
            XCTAssertEqual(key.step, step % 8)
        }
        XCTAssertEqual(request(step: 0).keys, request(step: 16).keys)
    }

    @MainActor func testRealFrameCacheUsesActualBytesAndEvictsLRU() {
        let palette = NinjaPixelActor.palette(style: "ice")
        let keys = (0..<3).map { NinjaPixelActor.Key(style: "ice", action: .wave, step: $0) }
        let first = NinjaActorRaster.frame(keys[0], palette: palette)!
        let cost = first.bytesPerRow * first.height
        let store = NinjaActorRasterStore(countLimit: 2, byteLimit: cost * 2)
        func batch(_ keys: [NinjaPixelActor.Key]) -> NinjaActorFrameResult {
            store.render(NinjaActorFrameRequest(keys: keys, palette: palette))!
        }
        let initial = batch([keys[0], keys[1], keys[0]])
        XCTAssertEqual(initial.generated, 2); XCTAssertEqual(initial.frames.count, 2)
        XCTAssertEqual(store.count, 2); XCTAssertEqual(store.bytes, cost * 2)
        XCTAssertEqual(batch([keys[0]]).generated, 0)
        XCTAssertEqual(batch([keys[2]]).generated, 1)
        XCTAssertEqual(batch([keys[0]]).generated, 0)
        XCTAssertEqual(batch([keys[1]]).generated, 1)
        XCTAssertLessThanOrEqual(store.bytes, cost * 2)
    }

    @MainActor func testPaletteCacheRetainsAtMostFourteenExactStyleKeys() {
        for style in 0..<40 { _ = NinjaPixelActor.palette(style: "unknown-\(style)") }
        XCTAssertEqual(NinjaPixelActor.diagnosticPaletteCount, 14)
    }
}
