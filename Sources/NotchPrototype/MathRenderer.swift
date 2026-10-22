import AppKit
import CoreImage
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
    private struct Job { let id=UUID();let key: String; let latex: String; let display: Bool; let startColor: String; let endColor: String }
    private var webView: PassiveMathWebView?
    private var ready = false
    private var pending = [Job]()
    private var inFlight: Job?
    private var jobTimeout:DispatchWorkItem?
    private var requested = Set<String>()
    private let images = NSCache<NSString,NSImage>()
    private var failed = Set<String>()
    private lazy var imageContext = CIContext(options:[.cacheIntermediates:false])
    var isInstalled:Bool {webView != nil}
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
        pending.removeAll();requested.removeAll();failed.removeAll()
        if let job=inFlight{requested.insert(job.key)}
    }

    func suspend() {
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
        if webView != nil,pending.count<128,requested.count<512,!failed.contains(key), requested.insert(key).inserted {
            pending.append(Job(key: key, latex: expression.latex, display: expression.display, startColor: start, endColor: end))
            processNext()
        }
        return nil
    }

    nonisolated func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Task { @MainActor [weak self] in self?.ready = true; self?.processNext() }
    }

    nonisolated func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        Task { @MainActor [weak self] in self?.suspend() }
    }

    nonisolated func webViewWebContentProcessDidTerminate(_ webView:WKWebView) {
        Task { @MainActor [weak self] in self?.suspend() }
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

    private func finish(_ job: Job, image: NSImage?) {
        guard inFlight?.id == job.id else { return }
        jobTimeout?.cancel();jobTimeout=nil
        // An evicted image must not start a render/evict loop during equation
        // refreshes. A new document or reopening resets the request budget.
        if let image,let tinted = tintedImage(image, startColor: job.startColor, endColor: job.endColor) { images.setObject(tinted,forKey:job.key as NSString,cost:Int(tinted.size.width*tinted.size.height*16)); onImageReady?(job.latex) }
        else {if failed.count>=256{failed.removeAll()};failed.insert(job.key)}
        inFlight = nil
        processNext()
    }

    private func tintedImage(_ image: NSImage, startColor: String, endColor: String) -> NSImage? {
        guard let data=image.tiffRepresentation,let input=CIImage(data:data),
              let start=UInt64(startColor.dropFirst(),radix:16),
              let end=UInt64(endColor.dropFirst(),radix:16),
              let filter=CIFilter(name:"CIColorMatrix") else{return nil}
        func color(_ value:UInt64)->CIColor {
            CIColor(red:CGFloat((value>>16)&255)/255,green:CGFloat((value>>8)&255)/255,blue:CGFloat(value&255)/255)
        }
        let zero=CIVector(x:0,y:0,z:0,w:0)
        filter.setValue(input,forKey:kCIInputImageKey)
        filter.setValue(zero,forKey:"inputRVector")
        filter.setValue(zero,forKey:"inputGVector")
        filter.setValue(zero,forKey:"inputBVector")
        filter.setValue(CIVector(x:1,y:0,z:0,w:0),forKey:"inputAVector")
        filter.setValue(CIVector(x:1,y:1,z:1,w:0),forKey:"inputBiasVector")
        guard let mask=filter.outputImage,
              let gradient=CIFilter(name:"CILinearGradient",parameters:[
                "inputPoint0":CIVector(x:input.extent.minX,y:input.extent.midY),
                "inputPoint1":CIVector(x:input.extent.maxX,y:input.extent.midY),
                "inputColor0":color(start),"inputColor1":color(end)])?.outputImage,
              let output=CIFilter(name:"CISourceInCompositing",parameters:[
                kCIInputImageKey:gradient,kCIInputBackgroundImageKey:mask])?.outputImage,
              let bitmap=imageContext.createCGImage(output,from:input.extent) else{return nil}
        return NSImage(cgImage:bitmap,size:image.size)
    }
}
