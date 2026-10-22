import AppKit

enum AttachmentImageEncoding {
    /// Own an independent NSImage container before handing it to the worker.
    /// Only the serial encoding queue uses this copy; its pixel data is read-only.
    final class Input: @unchecked Sendable {
        private let image:NSImage
        init(_ image:NSImage){self.image=image.copy() as! NSImage}
        func png()->Data? {AttachmentImageEncoding.png(image)}
    }

    /// Called on the serial attachment queue; preserve the original pixels.
    static func png(_ image:NSImage)->Data? {
        guard let bitmap=image.cgImage(forProposedRect:nil,context:nil,hints:nil) else{return nil}
        return NSBitmapImageRep(cgImage:bitmap).representation(using:.png,properties:[:])
    }
}
