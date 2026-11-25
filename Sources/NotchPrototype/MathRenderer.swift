import AppKit
import WebKit

struct MathExpression {
    let range: NSRange
    let latex: String
    let display: Bool
    private static let regex = try! NSRegularExpression(pattern: #"(?<!\\)(\$\$)(.+?)(?<!\\)\$\$|(?<!\\)\\\[(.+?)(?<!\\)\\\]|(?<!\\)\\\((.+?)(?<!\\)\\\)|(?<![\\$])\$(?![\s$])(.+?)(?<![\s\\])\$(?!\$)"#, options: [.dotMatchesLineSeparators])

    static func find(in text: String) -> [MathExpression] {
        let source = text as NSString
        guard text.contains("$") || text.contains("\\(") || text.contains("\\[") else{return []}
        return regex.matches(in: text, range: NSRange(location: 0, length: source.length)).compactMap { match in
            let group = (2...5).first { match.range(at: $0).location != NSNotFound }
            guard let group else { return nil }
            let latex = source.substring(with: match.range(at: group))
            guard !latex.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
            return MathExpression(range: match.range, latex: latex, display: group == 2 || group == 3)
        }
    }

    static func collapsedLines(_ lines: [KaiLine]) -> [KaiLine] {
        var output = [KaiLine](), index = 0
        while index < lines.count {
            let opener = lines[index].text.trimmingCharacters(in: .whitespaces)
            let closer = opener == "$$" ? "$$" : opener == "\\[" ? "\\]" : ""
            if !closer.isEmpty && lines[index].style != "code",
               let end = ((index + 1)..<lines.count).first(where: {
                   lines[$0].text.trimmingCharacters(in: .whitespaces) == closer
               }) {
                let body = lines[(index + 1)..<end].map(\.text).joined(separator: "\n")
                let text = opener == "$$" ? "$$\(body)$$" : "\\[\(body)\\]"
                output.append(KaiLine(text: text, style: "math", mathEnabled: true))
                index = end + 1
            } else {
                output.append(lines[index]); index += 1
            }
        }
        return output
    }

    static func containsRenderableMath(_ lines:[KaiLine])->Bool {
        guard lines.contains(where:{$0.style != "code" && $0.mathEnabled != false && ($0.text.contains("$") || $0.text.contains("\\(") || $0.text.contains("\\["))}) else{return false}
        return collapsedLines(lines).contains{$0.style != "code" && $0.mathEnabled != false && !find(in:$0.text).isEmpty}
    }
}

private final class PassiveMathWebView: WKWebView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

@MainActor final class MathRenderer: NSObject, WKNavigationDelegate {
    private struct Job { let id=UUID();let documentRevision: UInt64;let key: String; let latex: String; let display: Bool; let startColor: String; let endColor: String }
    private var webView: PassiveMathWebView?
    private var ready = false
    private var pending = [Job]()
    private var inFlight: Job?
    private var jobTimeout:DispatchWorkItem?
    private var requested = Set<String>()
    private let images = NSCache<NSString,NSImage>()
    private var failed = Set<String>()
    private let imagePipeline = MathImagePipeline()
    private var documentRevision: UInt64 = 0
    private var rasterizingJob: UUID?
    #if DEBUG
    private var diagnosticSnapshotTransportEnabled = false
    #endif
    var isInstalled: Bool {
        #if DEBUG
        if diagnosticSnapshotTransportEnabled { return true }
        #endif
        return webView != nil
    }
    var onImageReady: ((String) -> Void)?

    override init(){super.init();images.countLimit=256;images.totalCostLimit=24*1024*1024}

    @discardableResult func install(in host: NSView, below sibling: NSView)->Bool {
        guard webView == nil else { return false }
        let root = Bundle.main.resourceURL?.appendingPathComponent("katex")
            ?? URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Resources/katex")
        let page = root.appendingPathComponent("renderer.html")
        guard FileManager.default.fileExists(atPath: page.path) else { return false }
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let web = PassiveMathWebView(frame: NSRect(x: 0, y: 0, width: 700, height: 260), configuration: configuration)
        web.navigationDelegate = self
        web.underPageBackgroundColor = .clear
        web.alphaValue = 0.001
        host.addSubview(web, positioned: .below, relativeTo: sibling)
        webView = web
        web.loadFileURL(page, allowingReadAccessTo: root)
        return true
    }

    func beginDocument() {
        documentRevision &+= 1
        pending.removeAll();requested.removeAll();failed.removeAll()
        // A WebKit snapshot already in progress drains before changing its DOM.
        // A captured immutable image can retire immediately without publication.
        if let job = inFlight, rasterizingJob == job.id {
            imagePipeline.cancel()
            complete(job, bitmap: nil, size: .zero)
        }
    }

    func suspend() {
        #if DEBUG
        diagnosticSnapshotTransportEnabled = false
        #endif
        documentRevision &+= 1
        imagePipeline.cancel()
        rasterizingJob = nil
        jobTimeout?.cancel();jobTimeout=nil
        ready = false
        pending.removeAll()
        requested.removeAll()
        inFlight = nil
        webView?.navigationDelegate = nil
        webView?.stopLoading()
        webView?.removeFromSuperview()
        webView = nil
    }

    func image(for expression: MathExpression, startColor: NSColor, endColor: NSColor) -> NSImage? {
        func hex(_ color: NSColor) -> String {
            let rgb=color.usingColorSpace(.deviceRGB) ?? .white
            return String(format:"#%02x%02x%02x",Int(rgb.redComponent*255),Int(rgb.greenComponent*255),Int(rgb.blueComponent*255))
        }
        let start=hex(startColor),end=hex(endColor)
        let key = "\(expression.display)\u{0}\(start)\u{0}\(end)\u{0}\(expression.latex)"
        if let image = images.object(forKey:key as NSString) { return image }
        if isInstalled,pending.count<128,requested.count<512,!failed.contains(key), requested.insert(key).inserted {
            pending.append(Job(documentRevision: documentRevision, key: key, latex: expression.latex, display: expression.display, startColor: start, endColor: end))
            processNext()
        }
        return nil
    }

    nonisolated func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Task { @MainActor [weak self] in
            guard let self, self.webView === webView else { return }
            self.ready = true
            self.processNext()
        }
    }

    nonisolated func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        Task { @MainActor [weak self] in
            guard let self, self.webView === webView else { return }
            self.suspend()
        }
    }

    nonisolated func webViewWebContentProcessDidTerminate(_ webView:WKWebView) {
        Task { @MainActor [weak self] in
            guard let self, self.webView === webView else { return }
            self.suspend()
        }
    }

    private func processNext() {
        guard ready, inFlight == nil, !pending.isEmpty, let webView else { return }
        let job = pending.removeFirst()
        inFlight = job
        let timeout=DispatchWorkItem{[weak self] in self?.finish(job,image:nil)}
        jobTimeout=timeout;DispatchQueue.main.asyncAfter(deadline:.now()+5,execute:timeout)
        // JSON encoding provides a JavaScript string literal without interpolating TeX as code.
        guard let data = try? JSONSerialization.data(withJSONObject: [job.latex, job.display, "#ffffff"], options: [.fragmentsAllowed]),
              let arguments = String(data: data, encoding: .utf8) else { finish(job, image: nil); return }
        webView.evaluateJavaScript("renderMath(...\(arguments))") { [weak self] value, error in
            guard let self,self.inFlight?.id == job.id else { return }
            guard error == nil, value is [String: Any] else {
                self.finish(job, image: nil); return
            }
            self.snapshotWhenReady(job, from: webView, attempt: 0)
        }
    }

    private func snapshotWhenReady(_ job: Job, from webView: WKWebView, attempt: Int) {
        webView.evaluateJavaScript("({ready:document.fonts.status==='loaded',...mathSize()})") { [weak self] value, error in
            guard let self,self.inFlight?.id == job.id else { return }
            guard error == nil, let dimensions = value as? [String: Any],
                  let width = dimensions["width"] as? NSNumber, let height = dimensions["height"] as? NSNumber else {
                self.finish(job, image: nil); return
            }
            if dimensions["ready"] as? Bool != true && attempt < 30 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.03) { [weak self] in
                    self?.snapshotWhenReady(job, from: webView, attempt: attempt + 1)
                }
                return
            }
            let size = CGSize(width: min(680, max(1, width.doubleValue)), height: min(240, max(1, height.doubleValue)))
            let config = WKSnapshotConfiguration()
            config.rect = CGRect(origin: .zero, size: size)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { [weak self] in
                guard let self,self.inFlight?.id == job.id else{return}
                webView.takeSnapshot(with: config) { [weak self] image, _ in self?.finish(job, image: image) }
            }
        }
    }

    /// Capture a pixel representation while the snapshot remains on its owner actor.
    static func snapshotBitmap(_ image: NSImage) -> CGImage? {
        let bitmaps = image.representations.compactMap { ($0 as? NSBitmapImageRep)?.cgImage }
        if let largest = bitmaps.max(by: {
            UInt64($0.width) * UInt64($0.height) < UInt64($1.width) * UInt64($1.height)
        }) {
            return largest
        }
        let size = image.size
        guard size.width.isFinite, size.height.isFinite,
              size.width > 0, size.height > 0,
              size.width < CGFloat(Int.max), size.height < CGFloat(Int.max),
              let context = CGContext(
                data: nil, width: Int(ceil(size.width)), height: Int(ceil(size.height)),
                bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
              ) else { return nil }
        let graphics = NSGraphicsContext(cgContext: context, flipped: false)
        var rectangle = CGRect(origin: .zero, size: size)
        return image.cgImage(forProposedRect: &rectangle, context: graphics, hints: nil)
    }

    #if DEBUG
    func diagnosticInstallSnapshotTransport() {
        diagnosticSnapshotTransportEnabled = true
    }
    var diagnosticPendingSnapshotCount: Int { pending.count }
    var diagnosticInFlightSnapshotID: UUID? { inFlight?.id }
    var diagnosticRequestedKeyCount: Int { requested.count }
    /// Replace only WebKit transport in tests. Queue and publication use normal code.
    func diagnosticBeginNextSnapshot() -> UUID? {
        guard diagnosticSnapshotTransportEnabled, inFlight == nil, !pending.isEmpty else { return nil }
        let job = pending.removeFirst()
        inFlight = job
        return job.id
    }
    func diagnosticDeliverSnapshot(_ id: UUID, image: NSImage?) {
        guard let job = inFlight, job.id == id else { return }
        finish(job, image: image)
    }
    #endif

    private func finish(_ job: Job, image: NSImage?) {
        guard inFlight?.id == job.id else { return }
        guard job.documentRevision == documentRevision else {
            complete(job, bitmap: nil, size: .zero)
            return
        }
        // Keep AppKit representation selection on the snapshot owner actor.
        guard let image,
              let bitmap = Self.snapshotBitmap(image) else {
            imagePipeline.cancel()
            complete(job, bitmap: nil, size: .zero)
            return
        }
        let size = image.size
        rasterizingJob = job.id
        imagePipeline.submit(MathImageRequest(
            image: bitmap, startColor: job.startColor, endColor: job.endColor
        )) { [weak self] result in
            guard let self, self.inFlight?.id == job.id else { return }
            self.complete(job, bitmap: result?.image, size: size)
        }
    }

    private func complete(_ job: Job, bitmap: CGImage?, size: CGSize) {
        guard inFlight?.id == job.id else { return }
        jobTimeout?.cancel();jobTimeout=nil
        rasterizingJob = nil
        if job.documentRevision == documentRevision {
            // Retain the old request budget so eviction cannot create a render loop.
            if let bitmap {
                let image = NSImage(cgImage: bitmap, size: size)
                let cost = bitmap.bytesPerRow * bitmap.height + job.key.utf8.count
                images.setObject(image, forKey: job.key as NSString, cost: cost)
                onImageReady?(job.latex)
            } else {
                if failed.count >= 256 { failed.removeAll() }
                failed.insert(job.key)
            }
        }
        inFlight = nil
        processNext()
    }
}
