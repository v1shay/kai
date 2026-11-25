import AppKit
import QuartzCore

@MainActor
final class WorkspacePaneFeedback {
    private final class ToastView: NSVisualEffectView {
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
    }

    private struct Border {
        let layer: CAGradientLayer
        let path: CAShapeLayer
        var phase: String
        var revision: UUID
    }

    private var borders = [String: Border]()
    private var toast: NSView?

    func update(owner: String, phase: String, rect: CGRect, palette: () -> [NSColor], in host: NSView) {
        guard host.bounds.width > 0, host.bounds.height > 0 else { return }
        if phase != "loading", borders[owner] == nil { return }
        let frame = CGRect(x: rect.minX, y: host.isFlipped ? rect.minY : host.bounds.height - rect.maxY,
                           width: rect.width, height: rect.height).intersection(host.bounds)
        guard !frame.isNull, frame.width > 10, frame.height > 10 else { return }
        host.wantsLayer = true
        guard let parent = host.layer else { return }
        if borders[owner] == nil {
            let layer = CAGradientLayer()
            layer.colors = palette().map(\.cgColor)
            layer.startPoint = CGPoint(x: 0.05, y: 0.1)
            layer.endPoint = CGPoint(x: 0.95, y: 0.9)
            let path = CAShapeLayer()
            path.fillColor = nil
            path.strokeColor = NSColor.white.cgColor
            path.lineWidth = 1.6
            path.lineCap = .round
            layer.mask = path
            layer.zPosition = 100
            parent.addSublayer(layer)
            borders[owner] = Border(layer: layer, path: path, phase: "", revision: UUID())
        }
        guard var border = borders[owner] else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        border.layer.frame = frame.insetBy(dx: 2, dy: 2)
        border.path.frame = border.layer.bounds
        let outline = border.path.bounds.insetBy(dx: 1, dy: 1)
        border.path.path = CGPath(roundedRect: outline, cornerWidth: 11, cornerHeight: 11, transform: nil)
        CATransaction.commit()
        guard border.phase != phase else { return }
        border.phase = phase
        border.revision = UUID()
        borders[owner] = border
        let revision = border.revision
        let reduced = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if phase == "loading" {
            border.layer.removeAnimation(forKey: "finish")
            border.layer.opacity = 1
            border.path.lineDashPattern = [0.5, 6]
            if !reduced {
                let flow = CABasicAnimation(keyPath: "lineDashPhase")
                flow.fromValue = 0
                flow.toValue = -13
                flow.duration = 0.68
                flow.repeatCount = .infinity
                border.path.add(flow, forKey: "flow")
            }
            scheduleRemoval(owner, revision: revision, after: 24)
        } else {
            let outgoingDots = CAShapeLayer()
            outgoingDots.frame = border.path.frame
            outgoingDots.path = border.path.path
            outgoingDots.fillColor = nil
            outgoingDots.strokeColor = NSColor.white.cgColor
            outgoingDots.lineWidth = border.path.lineWidth
            outgoingDots.lineCap = .round
            outgoingDots.lineDashPattern = border.path.lineDashPattern
            outgoingDots.lineDashPhase = (border.path.presentation() ?? border.path).lineDashPhase

            border.path.removeAnimation(forKey: "flow")
            if phase == "ready" {
                border.path.lineDashPattern = nil
                if !reduced {
                    border.path.addSublayer(outgoingDots)
                    let dots = CABasicAnimation(keyPath: "opacity")
                    dots.fromValue = 1
                    dots.toValue = 0
                    dots.duration = 0.20
                    outgoingDots.opacity = 0
                    outgoingDots.add(dots, forKey: "dissolve")
                    let line = CABasicAnimation(keyPath: "strokeEnd")
                    line.fromValue = 0
                    line.toValue = 1
                    line.duration = 0.24
                    line.timingFunction = KaiMotion.interactionCurve
                    border.path.add(line, forKey: "complete")
                }
            }
            let fade = CAKeyframeAnimation(keyPath: "opacity")
            fade.values = [1, 1, 0]
            fade.keyTimes = [0, 0.55, 1]
            fade.duration = reduced ? 0.15 : 0.68
            border.layer.opacity = 0
            border.layer.add(fade, forKey: "finish")
            scheduleRemoval(owner, revision: revision, after: fade.duration)
        }
    }

    private func scheduleRemoval(_ owner: String, revision: UUID, after duration: TimeInterval) {
        DispatchQueue.main.asyncAfter(deadline: .now() + duration) { [weak self] in
            guard let self, self.borders[owner]?.revision == revision else { return }
            self.borders.removeValue(forKey: owner)?.layer.removeFromSuperlayer()
        }
    }

    func notify(_ text: String, error: Bool, in host: NSView) {
        toast?.removeFromSuperview()
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = AppTypography.ui(ofSize: 12, weight: .medium)
        label.textColor = .labelColor
        label.maximumNumberOfLines = 3
        let width = min(max(200, CGFloat(text.count) * 6 + 28), min(440, host.bounds.width - 32))
        guard width > 32 else { return }
        label.preferredMaxLayoutWidth = width - 24
        let height = max(36, label.fittingSize.height + 18)
        let y = host.isFlipped ? host.bounds.maxY - height - 20 : host.bounds.minY + 20
        let view = ToastView(frame: CGRect(x: host.bounds.midX - width / 2, y: y, width: width, height: height))
        view.material = .popover
        view.blendingMode = .withinWindow
        view.state = .active
        view.wantsLayer = true
        view.layer?.cornerRadius = 10
        view.layer?.masksToBounds = true
        view.layer?.borderColor = (error ? NSColor.systemOrange : NSColor.controlAccentColor).withAlphaComponent(0.55).cgColor
        view.layer?.borderWidth = 1
        view.autoresizingMask = [.minXMargin, .maxXMargin, host.isFlipped ? .minYMargin : .maxYMargin]
        label.frame = view.bounds.insetBy(dx: 12, dy: 8)
        view.addSubview(label)
        host.addSubview(view, positioned: .above, relativeTo: nil)
        toast = view
        let fade = CAKeyframeAnimation(keyPath: "opacity")
        fade.values = [0, 1, 1, 0]
        fade.keyTimes = [0, 0.05, 0.9, 1]
        fade.duration = 4
        view.layer?.opacity = 0
        view.layer?.add(fade, forKey: "toast")
        DispatchQueue.main.asyncAfter(deadline: .now() + fade.duration) { [weak self, weak view] in
            guard let self, let view, self.toast === view else { return }
            view.removeFromSuperview()
            self.toast = nil
        }
    }
}
