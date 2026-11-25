import AppKit
import Foundation

let destination = CommandLine.arguments[1]
let sourcePath = CommandLine.arguments.count > 2
    ? CommandLine.arguments[2]
    : "assets/wallpapers/bonsai-sakura.png"
guard let artwork = NSImage(contentsOfFile: sourcePath), artwork.size.width > 0, artwork.size.height > 0 else {
    fatalError("Unable to load app-icon artwork: \(sourcePath)")
}
try FileManager.default.createDirectory(atPath: destination, withIntermediateDirectories: true)
let cropSide = min(artwork.size.width, artwork.size.height)
let crop = NSRect(
    x: (artwork.size.width - cropSide) / 2,
    y: (artwork.size.height - cropSide) / 2,
    width: cropSide,
    height: cropSide
)
for points in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        try autoreleasepool {
            let pixels = points * scale
            guard let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
                bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
            ), let context = NSGraphicsContext(bitmapImageRep: bitmap) else {
                fatalError("Unable to allocate app-icon raster")
            }
            NSGraphicsContext.saveGraphicsState()
            defer { NSGraphicsContext.restoreGraphicsState() }
            NSGraphicsContext.current = context
            context.imageInterpolation = .high
            let side = CGFloat(pixels)
            NSRect(x: 0, y: 0, width: side, height: side).fill(using: .clear)
            let rect = NSRect(x: side * 0.07, y: side * 0.07, width: side * 0.86, height: side * 0.86)
            NSBezierPath(roundedRect: rect, xRadius: side * 0.19, yRadius: side * 0.19).addClip()
            artwork.draw(in: rect, from: crop, operation: .copy, fraction: 1)
            let filename = "icon_\(points)x\(points)\(scale == 2 ? "@2x" : "").png"
            guard let png = bitmap.representation(using: .png, properties: [:]) else {
                fatalError("Unable to encode app-icon raster")
            }
            try png.write(to: URL(fileURLWithPath: destination + "/" + filename))
        }
    }
}
