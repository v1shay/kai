import Foundation
import CoreGraphics

/// Immutable palette and private bitmap contexts. This file performs no AppKit calls.
enum NinjaActorRaster {
    struct Palette: @unchecked Sendable {
        let suit: CGColor, shadow: CGColor, rim: CGColor
        let gold: CGColor, ink: CGColor, face: CGColor
        let inkStroke: CGColor, goldShade: CGColor, rimSeam: CGColor
    }

    static func frame(_ key: NinjaPixelActor.Key, palette: Palette) -> CGImage? {
        let n = key.step, action = key.action, left = key.left
        guard let c = CGContext(data: nil, width: 144, height: 168, bitsPerComponent: 8,
            bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        c.translateBy(x: left ? 144 : 0, y: 168)
        c.scaleBy(x: left ? -3 : 3, y: -3)
        let suit = palette.suit, shadow = palette.shadow, rim = palette.rim
        let gold = palette.gold, ink = palette.ink, face = palette.face
        let wave = sin(Double(n) * .pi / 4)
        let moving = action == .walk || action == .run
        let stride = action == .run ? 0.74 : 0.44
        let crouch = action == .tuck ? 3.5 : action == .inspect ? 2.4 : action == .bow ? (1 - cos(Double(n) * .pi / 4)) * 1.8 : 0
        c.translateBy(x: 0, y: crouch + (moving ? abs(wave) : 0))
        func fill(_ path: CGPath, _ color: CGColor) {
            c.addPath(path)
            c.setFillColor(color)
            c.fillPath()
        }
        func plate(_ rect: CGRect, radius: CGFloat, upper: CGColor, lower: CGColor) {
            let path = CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil)
            c.saveGState()
            c.addPath(path)
            c.clip()
            let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                colors: [upper, lower] as CFArray, locations: [0, 1])!
            c.drawLinearGradient(gradient, start: rect.origin,
                end: CGPoint(x: rect.maxX, y: rect.maxY), options: [])
            c.restoreGState()
            c.addPath(path)
            c.setStrokeColor(palette.inkStroke)
            c.setLineWidth(0.65)
            c.strokePath()
        }
        func seam(_ points: [CGPoint], _ color: CGColor, width: CGFloat) {
            c.beginPath()
            c.addLines(between: points)
            c.setStrokeColor(color)
            c.setLineWidth(width)
            c.setLineCap(.round)
            c.setLineJoin(.round)
            c.strokePath()
        }
        func leg(_ x: CGFloat, angle: CGFloat, far: Bool) {
            c.saveGState()
            c.translateBy(x: x, y: 39)
            c.rotate(by: angle)
            plate(CGRect(x: -3.5, y: 0, width: 7, height: 12), radius: 0.8,
                upper: far ? shadow : suit, lower: shadow)
            plate(CGRect(x: -3.7, y: 9, width: 8.4, height: 4), radius: 0.7,
                upper: shadow, lower: ink)
            c.restoreGState()
        }
        func arm(_ x: CGFloat, angle: CGFloat, far: Bool) {
            c.saveGState()
            c.translateBy(x: x, y: 28)
            c.rotate(by: angle)
            plate(CGRect(x: -3, y: -1.5, width: 6, height: 12), radius: 2.3,
                upper: far ? suit : rim, lower: shadow)
            let hand = CGMutablePath()
            hand.addArc(center: CGPoint(x: 0, y: 11.5), radius: 2.7,
                startAngle: 0.35, endAngle: 5.4, clockwise: false)
            c.addPath(hand)
            c.setLineWidth(1.55)
            c.setStrokeColor(shadow)
            c.strokePath()
            c.restoreGState()
        }

        let legSwing = moving ? wave * stride : action == .climb ? wave * 0.52 : 0
        let frontLeg = action == .tuck ? -0.9 : action == .leap ? -0.35 - abs(wave) * 0.4 : legSwing
        leg(19, angle: action == .tuck ? 0.85 : -legSwing, far: true)
        leg(28, angle: frontLeg, far: false)
        let farArm = action == .climb ? 2.25 + wave * 0.35 : legSwing * 0.9 + 0.15
        arm(15, angle: farArm, far: true)
        plate(CGRect(x: 15, y: 25, width: 18, height: 16), radius: 2,
            upper: rim, lower: shadow)
        seam([CGPoint(x: 17, y: 27), CGPoint(x: 29, y: 36)], ink, width: 2.4)
        seam([CGPoint(x: 32, y: 27), CGPoint(x: 20, y: 38)], gold, width: 1.15)
        seam([CGPoint(x: 16, y: 37), CGPoint(x: 32, y: 37)], ink, width: 2.1)
        fill(CGPath(ellipseIn: CGRect(x: 22, y: 35.5, width: 4, height: 3), transform: nil), gold)
        let nearArm: CGFloat
        switch action {
        case .wave: nearArm = -2.35 + wave * 0.34
        case .climb: nearArm = -2.3 + wave * 0.40
        case .leap: nearArm = -1.65 - abs(wave) * 0.45
        case .tuck: nearArm = 0.78 + wave * 0.16
        case .inspect: nearArm = -0.95 + wave * 0.19
        case .bow: nearArm = -0.24 - abs(wave) * 0.34
        default: nearArm = -legSwing * 0.95 - 0.14
        }
        arm(33, angle: nearArm, far: false)

        c.saveGState()
        c.translateBy(x: 24, y: 17)
        c.rotate(by: action == .bow ? crouch * 0.10 : moving ? wave * 0.05 : 0)
        plate(CGRect(x: -12, y: -9, width: 24, height: 20), radius: 6,
            upper: rim, lower: shadow)
        plate(CGRect(x: -9, y: -2, width: 18, height: 8), radius: 2,
            upper: face, lower: palette.goldShade)
        let blink: CGFloat = action == .rest && n == 7 ? 0.7 : 2.5
        for x: CGFloat in [-5.3, 3.4] {
            fill(CGPath(ellipseIn: CGRect(x: x, y: 0.4, width: 2, height: blink), transform: nil), ink)
        }
        seam([CGPoint(x: -6.5, y: -1), CGPoint(x: -2.3, y: 0),
              CGPoint(x: 2.3, y: 0), CGPoint(x: 6.5, y: -1)], ink, width: 0.75)
        plate(CGRect(x: -10.5, y: 4.3, width: 21, height: 6.6), radius: 2.5,
            upper: suit, lower: shadow)
        seam([CGPoint(x: -8, y: 6.6), CGPoint(x: 0, y: 7.6), CGPoint(x: 8, y: 5.7)],
             palette.rimSeam, width: 0.85)
        c.restoreGState()
        return c.makeImage()
    }
}
