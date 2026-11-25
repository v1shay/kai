import AppKit

/// Coordinates retirement of the actual notch. No screen-sized overlay, window
/// capture, or workspace contour participates in an automatic app switch.
@MainActor final class WorkspaceStatusMorph {
    private var retire: DispatchWorkItem?
    private var revision = 0
    private(set) var active = false
    func cancel() {
        revision += 1; retire?.cancel(); retire = nil; active = false
    }
    func retract(after duration: TimeInterval, completion: @escaping () -> Void) {
        cancel(); active = true
        let token = revision
        let finish = DispatchWorkItem { [weak self] in
            guard let self, self.revision == token else { return }
            self.retire = nil; self.active = false; completion()
        }
        retire = finish
        DispatchQueue.main.asyncAfter(deadline: .now() + duration, execute: finish)
    }
}
