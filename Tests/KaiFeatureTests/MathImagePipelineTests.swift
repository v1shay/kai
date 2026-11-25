import AppKit
import CoreImage
import CoreText
import XCTest
import WebKit
@testable import NotchPrototype

final class MathImagePipelineTests: XCTestCase {
    @MainActor
    func testSharedWorkerKeepsOnlyTheLatestPendingRequest() throws {
        let image = try fixture(scale: 1, space: CGColorSpaceCreateDeviceRGB(), alpha: 1)
        let bitmap = try XCTUnwrap(MathRenderer.snapshotBitmap(image))
        var jobs = [(MathImageRequest, MathImagePipeline.Completion)]()
        let pipeline = MathImagePipeline(execute: { jobs.append(($0, $1)) })
        var published = [String]()
        for index in 0..<100 {
            let color = String(format: "#%06x", index)
            pipeline.submit(MathImageRequest(image: bitmap, startColor: color, endColor: "#ffffff")) { _ in
                published.append(color)
            }
            XCTAssertLessThanOrEqual(pipeline.pendingCount, 1)
            XCTAssertLessThanOrEqual(MathImagePipeline.diagnosticActiveWorkerCount, 1)
            XCTAssertLessThanOrEqual(MathImagePipeline.diagnosticQueuedRendererCount, 1)
        }
        XCTAssertEqual(jobs.count, 1)
        jobs.removeFirst().1(nil)
        XCTAssertTrue(published.isEmpty, "An older active completion must not publish")
        XCTAssertEqual(jobs.count, 1)
        XCTAssertEqual(jobs[0].0.startColor, "#000063")
        jobs.removeFirst().1(nil)
        XCTAssertEqual(published, ["#000063"])
        XCTAssertEqual(MathImagePipeline.diagnosticActiveWorkerCount, 0)
        pipeline.cancel()
    }

    @MainActor
    func testCancellationDropsQueuedWorkAndLatePublication() throws {
        let image = try fixture(scale: 1, space: CGColorSpaceCreateDeviceRGB(), alpha: 1)
        let bitmap = try XCTUnwrap(MathRenderer.snapshotBitmap(image))
        var jobs = [MathImagePipeline.Completion]()
        let pipeline = MathImagePipeline(execute: { _, done in jobs.append(done) })
        var publications = 0
        let request = MathImageRequest(image: bitmap, startColor: "#ffffff", endColor: "#00ff00")
        pipeline.submit(request) { _ in publications += 1 }
        pipeline.submit(request) { _ in publications += 1 }
        pipeline.cancel()
        XCTAssertEqual(pipeline.pendingCount, 0)
        XCTAssertEqual(MathImagePipeline.diagnosticQueuedRendererCount, 0)
        jobs.removeFirst()(nil)
        XCTAssertTrue(jobs.isEmpty)
        XCTAssertEqual(publications, 0)
        XCTAssertEqual(MathImagePipeline.diagnosticActiveWorkerCount, 0)
    }

    @MainActor
    func testDocumentReplacementCanRequestTheSameEquationAfterAnOldSnapshot() async throws {
        let renderer = MathRenderer()
        renderer.diagnosticInstallSnapshotTransport()
        let expression = MathExpression(range: NSRange(location: 0, length: 3), latex: "x^2", display: false)
        let image = try fixture(scale: 2, space: CGColorSpaceCreateDeviceRGB(), alpha: 1)
        var publications = 0
        let ready = expectation(description: "Current document image")
        renderer.onImageReady = { _ in publications += 1; ready.fulfill() }
        XCTAssertNil(renderer.image(for: expression, startColor: .white, endColor: .green))
        let old = try XCTUnwrap(renderer.diagnosticBeginNextSnapshot())
        renderer.beginDocument()
        XCTAssertNil(renderer.image(for: expression, startColor: .white, endColor: .green))
        XCTAssertEqual(renderer.diagnosticPendingSnapshotCount, 1)
        renderer.diagnosticDeliverSnapshot(old, image: image)
        XCTAssertEqual(publications, 0)
        let current = try XCTUnwrap(renderer.diagnosticBeginNextSnapshot())
        XCTAssertNotEqual(current, old)
        renderer.diagnosticDeliverSnapshot(current, image: image)
        await fulfillment(of: [ready], timeout: 5)
        XCTAssertEqual(publications, 1)
        let cached = try XCTUnwrap(renderer.image(for: expression, startColor: .white, endColor: .green))
        XCTAssertEqual(cached.size, image.size)
        renderer.suspend()
    }

    @MainActor
    func testSuspendRejectsAnAlreadyCapturedImageAndAllowsTheNextDocument() async throws {
        let renderer = MathRenderer()
        renderer.diagnosticInstallSnapshotTransport()
        let expression = MathExpression(range: NSRange(location: 0, length: 1), latex: "x", display: true)
        let image = try fixture(scale: 1, space: CGColorSpaceCreateDeviceRGB(), alpha: 0.5)
        var publications = 0
        renderer.onImageReady = { _ in publications += 1 }
        _ = renderer.image(for: expression, startColor: .white, endColor: .green)
        let old = try XCTUnwrap(renderer.diagnosticBeginNextSnapshot())
        renderer.diagnosticDeliverSnapshot(old, image: image)
        renderer.suspend()
        for _ in 0..<1000 {
            if MathImagePipeline.diagnosticActiveWorkerCount == 0 { break }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
        XCTAssertEqual(MathImagePipeline.diagnosticActiveWorkerCount, 0)
        XCTAssertEqual(publications, 0)
        XCTAssertNil(renderer.diagnosticInFlightSnapshotID)
        renderer.diagnosticInstallSnapshotTransport()
        renderer.beginDocument()
        XCTAssertNil(renderer.image(for: expression, startColor: .white, endColor: .green))
        XCTAssertEqual(renderer.diagnosticPendingSnapshotCount, 1)
        renderer.suspend()
    }

    @MainActor
    func testSnapshotFailureDoesNotBlockAnotherExpressionOrANewDocumentRetry() throws {
        let renderer = MathRenderer()
        renderer.diagnosticInstallSnapshotTransport()
        let first = MathExpression(range: NSRange(location: 0, length: 1), latex: "x", display: false)
        let second = MathExpression(range: NSRange(location: 0, length: 1), latex: "y", display: false)
        _ = renderer.image(for: first, startColor: .white, endColor: .green)
        let failed = try XCTUnwrap(renderer.diagnosticBeginNextSnapshot())
        renderer.diagnosticDeliverSnapshot(failed, image: nil)
        XCTAssertNil(renderer.image(for: first, startColor: .white, endColor: .green))
        XCTAssertEqual(renderer.diagnosticPendingSnapshotCount, 0)
        _ = renderer.image(for: second, startColor: .white, endColor: .green)
        XCTAssertEqual(renderer.diagnosticPendingSnapshotCount, 1)
        renderer.beginDocument()
        _ = renderer.image(for: first, startColor: .white, endColor: .green)
        XCTAssertEqual(renderer.diagnosticPendingSnapshotCount, 1)
        renderer.suspend()
    }

    @MainActor
    func testDirectSnapshotPixelsMatchLegacyTIFFAcrossProfilesScalesAndAntialiasing() async throws {
        let spaces = [
            CGColorSpaceCreateDeviceRGB(),
            try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB)),
            try XCTUnwrap(CGColorSpace(name: CGColorSpace.displayP3))
        ]
        let context = CIContext(options: [.cacheIntermediates: false])
        let pipeline = MathImagePipeline()
        var cases = 0
        for space in spaces {
            for scale in [1, 2, 3] {
                for alpha: CGFloat in [0.35, 1] {
                    let image = try fixture(scale: scale, space: space, alpha: alpha)
                    for (start, end) in [("#ffffff", "#36c8ef"), ("#e38a41", "#814dd9")] {
                        let captured = try XCTUnwrap(MathRenderer.snapshotBitmap(image))
                        let legacySource = try fixture(scale: scale, space: space, alpha: alpha)
                        let expected = try legacyTint(legacySource, start: start, end: end, context: context)
                        let ready = expectation(description: "Worker bitmap \(cases)")
                        var result: MathImageResult?
                        pipeline.submit(MathImageRequest(
                            image: captured,
                            startColor: start, endColor: end
                        )) { value in result = value; ready.fulfill() }
                        await fulfillment(of: [ready], timeout: 5)
                        let actual = try XCTUnwrap(result).image
                        XCTAssertEqual(actual.width, expected.width)
                        XCTAssertEqual(actual.height, expected.height)
                        XCTAssertEqual(try rgba(actual), try rgba(expected), "scale=\(scale), alpha=\(alpha), profile=\(String(describing: space.name))")
                        print("KAI_MATH_PIXEL case=\(cases) input=\(captured.width)x\(captured.height) output=\(actual.width)x\(actual.height) profile=\(String(describing: space.name)) metal_context=\(result?.metalContextSelected == true)")
                        cases += 1
                    }
                }
            }
        }
        XCTAssertEqual(cases, 36)
        pipeline.cancel()
    }

    @MainActor
    func testColdGeneratedAndMultipleRepresentationSnapshotsMatchIndependentLegacyImages() async throws {
        let context = CIContext(options: [.cacheIntermediates: false])
        let pipeline = MathImagePipeline()
        let configurations: [(Bool, [Int], Bool)] = [
            (false, [], false),
            (true, [1, 2], false),
            (true, [2, 1], false),
            (true, [1, 3, 2], false),
            (true, [3, 2, 1], false),
            (true, [2, 1], true)
        ]
        for (multiple, scales, varyingDensity) in configurations {
            let source = try representationFixture(multiple: multiple, scales: scales, varyingDensity: varyingDensity)
            let expectedSource = try representationFixture(multiple: multiple, scales: scales, varyingDensity: varyingDensity)
            let captured = try XCTUnwrap(MathRenderer.snapshotBitmap(source))
            let expected = try legacyTint(expectedSource, start: "#ffffff", end: "#36c8ef", context: context)
            let ready = expectation(description: "Independent representation fixture")
            var result: MathImageResult?
            pipeline.submit(MathImageRequest(image: captured, startColor: "#ffffff", endColor: "#36c8ef")) {
                result = $0
                ready.fulfill()
            }
            await fulfillment(of: [ready], timeout: 5)
            let actual = try XCTUnwrap(result).image
            XCTAssertEqual(actual.width, expected.width)
            XCTAssertEqual(actual.height, expected.height)
            XCTAssertEqual(try rgba(actual), try rgba(expected), "multiple=\(multiple), scales=\(scales), varyingDensity=\(varyingDensity)")
        }
        pipeline.cancel()
    }


    @MainActor
    func testActualKaTeXWebKitSnapshotsMatchLegacyTIFFWithoutPresentingAWindow() async throws {
        let resourceRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/NotchPrototype/Resources/katex")
        let page = resourceRoot.appendingPathComponent("renderer.html")
        XCTAssertTrue(FileManager.default.fileExists(atPath: page.path))
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let web = WKWebView(frame: CGRect(x: 0, y: 0, width: 700, height: 260), configuration: configuration)
        web.underPageBackgroundColor = .clear
        web.alphaValue = 0.001
        let window = NSWindow(contentRect: web.frame, styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = web
        let loaded = expectation(description: "Real bundled KaTeX page")
        let delegate = MathSnapshotNavigationProbe(loaded: loaded)
        web.navigationDelegate = delegate
        defer {
            web.navigationDelegate = nil
            web.stopLoading()
            window.contentView = nil
            window.close()
        }
        web.loadFileURL(page, allowingReadAccessTo: resourceRoot)
        await fulfillment(of: [loaded], timeout: 10)
        XCTAssertNil(delegate.error)
        let referenceContext = CIContext(options: [.cacheIntermediates: false])
        let pipeline = MathImagePipeline()
        defer { pipeline.cancel() }
        for (latex, display) in [("x^2+y^2", false), (#"\frac{a+b}{\sqrt{x}}"#, true), (#"\int_0^\infty e^{-x^2}\,dx"#, true)] {
            let data = try JSONSerialization.data(withJSONObject: [latex, display, "#ffffff"])
            let arguments = try XCTUnwrap(String(data: data, encoding: .utf8))
            _ = try await web.evaluateJavaScript("renderMath(...\(arguments))")
            _ = try await web.callAsyncJavaScript("await document.fonts.ready; return true;", arguments: [:], in: nil, contentWorld: .page)
            let measured = try await web.evaluateJavaScript("mathSize()")
            let dimensions = try XCTUnwrap(measured as? [String: NSNumber])
            let size = CGSize(width: min(680, max(1, try XCTUnwrap(dimensions["width"]).doubleValue)), height: min(240, max(1, try XCTUnwrap(dimensions["height"]).doubleValue)))
            let snapshotConfiguration = WKSnapshotConfiguration()
            snapshotConfiguration.rect = CGRect(origin: .zero, size: size)
            try await Task.sleep(nanoseconds: 80_000_000)
            let snapshot: NSImage = try await withCheckedThrowingContinuation { continuation in
                web.takeSnapshot(with: snapshotConfiguration) { image, error in
                    if let error { continuation.resume(throwing: error) }
                    else if let image { continuation.resume(returning: image) }
                    else { continuation.resume(throwing: NSError(domain: "KaiMathSnapshot", code: 1)) }
                }
            }
            let captured = try XCTUnwrap(MathRenderer.snapshotBitmap(snapshot))
            let expected = try legacyTint(snapshot, start: "#ffffff", end: "#36c8ef", context: referenceContext)
            let ready = expectation(description: "Actual WK immutable worker bitmap")
            var result: MathImageResult?
            pipeline.submit(MathImageRequest(image: captured, startColor: "#ffffff", endColor: "#36c8ef")) {
                result = $0
                ready.fulfill()
            }
            await fulfillment(of: [ready], timeout: 5)
            let actual = try XCTUnwrap(result).image
            XCTAssertEqual(actual.width, expected.width)
            XCTAssertEqual(actual.height, expected.height)
            XCTAssertEqual(try rgba(actual), try rgba(expected), latex)
            XCTAssertEqual(snapshot.size, size)
            print("KAI_REAL_WK_SNAPSHOT display=\(display) captured=\(captured.width)x\(captured.height) logical=\(snapshot.size) reps=\(snapshot.representations.map { String(describing: type(of: $0)) }) metal_context=\(result?.metalContextSelected == true)")
        }
    }

    @MainActor
    private func representationFixture(multiple: Bool, scales: [Int] = [1, 2], varyingDensity: Bool = false) throws -> NSImage {
        if multiple {
            let image = NSImage(size: CGSize(width: 96, height: 32))
            for scale in scales {
                let bitmap = try XCTUnwrap(MathRenderer.snapshotBitmap(
                    fixture(scale: scale, space: CGColorSpaceCreateDeviceRGB(), alpha: 0.7)
                ))
                let representation = NSBitmapImageRep(cgImage: bitmap)
                representation.size = varyingDensity ? CGSize(width: 96 * scale, height: 32 * scale) : image.size
                image.addRepresentation(representation)
            }
            return image
        }
        return NSImage(size: CGSize(width: 96, height: 32), flipped: false) { _ in
            NSColor.white.withAlphaComponent(0.7).setFill()
            NSBezierPath(ovalIn: CGRect(x: 3.25, y: 3.5, width: 22.5, height: 21.25)).fill()
            NSAttributedString(string: "x²+∫y", attributes: [
                .font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.white
            ]).draw(at: CGPoint(x: 27, y: 8))
            return true
        }
    }

    @MainActor
    private func fixture(scale: Int, space: CGColorSpace, alpha: CGFloat) throws -> NSImage {
        let width = 96 * scale, height = 32 * scale
        let context = try XCTUnwrap(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
        context.setShouldAntialias(true)
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: alpha))
        context.fillEllipse(in: CGRect(x: 2.25, y: 2.5, width: 18.75, height: 19.25))
        context.setFillColor(try XCTUnwrap(CGColor(colorSpace: space, components: [0.92, 0.07, 0.45, alpha])))
        context.fill(CGRect(x: 77.25, y: 2.25, width: 12.5, height: 16.5))
        context.setTextDrawingMode(.fill)
        context.textPosition = CGPoint(x: 24, y: 8)
        let font = CTFontCreateWithName("TimesNewRomanPSMT" as CFString, 14, nil)
        let line = CTLineCreateWithAttributedString(NSAttributedString(
            string: "x²+∫y", attributes: [
                NSAttributedString.Key(kCTFontAttributeName as String): font,
                NSAttributedString.Key(kCTForegroundColorAttributeName as String): CGColor(gray: 1, alpha: alpha)
            ]
        ))
        CTLineDraw(line, context)
        return NSImage(cgImage: try XCTUnwrap(context.makeImage()), size: CGSize(width: 96, height: 32))
    }

    private func rgba(_ image: CGImage) throws -> Data {
        let width = image.width, height = image.height
        let context = try XCTUnwrap(CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB)),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.draw(image, in: CGRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)))
        return Data(bytes: try XCTUnwrap(context.data), count: width * height * 4)
    }

    @MainActor
    private func legacyTint(_ image: NSImage, start: String, end: String, context: CIContext) throws -> CGImage {
        let input = try XCTUnwrap(CIImage(data: try XCTUnwrap(image.tiffRepresentation)))
        let a = try XCTUnwrap(UInt64(start.dropFirst(), radix: 16))
        let b = try XCTUnwrap(UInt64(end.dropFirst(), radix: 16))
        func color(_ value: UInt64) -> CIColor {
            CIColor(red: CGFloat((value >> 16) & 255) / 255,
                    green: CGFloat((value >> 8) & 255) / 255,
                    blue: CGFloat(value & 255) / 255)
        }
        let filter = try XCTUnwrap(CIFilter(name: "CIColorMatrix"))
        let zero = CIVector(x: 0, y: 0, z: 0, w: 0)
        filter.setValue(input, forKey: kCIInputImageKey)
        filter.setValue(zero, forKey: "inputRVector")
        filter.setValue(zero, forKey: "inputGVector")
        filter.setValue(zero, forKey: "inputBVector")
        filter.setValue(CIVector(x: 1, y: 0, z: 0, w: 0), forKey: "inputAVector")
        filter.setValue(CIVector(x: 1, y: 1, z: 1, w: 0), forKey: "inputBiasVector")
        let mask = try XCTUnwrap(filter.outputImage)
        let gradient = try XCTUnwrap(CIFilter(name: "CILinearGradient", parameters: [
            "inputPoint0": CIVector(x: input.extent.minX, y: input.extent.midY),
            "inputPoint1": CIVector(x: input.extent.maxX, y: input.extent.midY),
            "inputColor0": color(a), "inputColor1": color(b)
        ])?.outputImage)
        let output = try XCTUnwrap(CIFilter(name: "CISourceInCompositing", parameters: [
            kCIInputImageKey: gradient, kCIInputBackgroundImageKey: mask
        ])?.outputImage)
        return try XCTUnwrap(context.createCGImage(output, from: input.extent))
    }
}

@MainActor
private final class MathSnapshotNavigationProbe: NSObject, WKNavigationDelegate {
    let loaded: XCTestExpectation
    var error: Error?
    init(loaded: XCTestExpectation) { self.loaded = loaded }
    nonisolated func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { Task { @MainActor in loaded.fulfill() } }
    nonisolated func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { Task { @MainActor in self.error = error; loaded.fulfill() } }
    nonisolated func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { Task { @MainActor in self.error = error; loaded.fulfill() } }
}
