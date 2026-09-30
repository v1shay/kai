import AppKit
import Foundation

// Compile with the copied IndicatorScenes.swift; this keeps exports in sync with
// the exact scene definitions used by the native animation engine.
let root = URL(fileURLWithPath: CommandLine.arguments.dropFirst().first ?? "handoff/notch-indicator")
let svgDirectory = root.appendingPathComponent("svg")
try FileManager.default.createDirectory(at: svgDirectory, withIntermediateDirectories: true)

func number(_ value: CGFloat) -> String { String(format: "%.3f", Double(value)) }
func number(_ value: Double) -> String { String(format: "%.3f", value) }

func poseObject(_ pose: IndicatorPose) -> [String: Any] {
    ["x": Double(pose.x), "y": Double(pose.y), "width": Double(pose.width),
     "height": Double(pose.height), "radius": Double(pose.radius),
     "rotation": Double(pose.rotation), "opacity": Double(pose.opacity),
     "shape": String(describing: pose.shape), "lineWidth": Double(pose.lineWidth)]
}

func svgElement(_ pose: IndicatorPose) -> String {
    guard pose.opacity > 0 else { return "" }
    let w = pose.width, h = pose.height
    let angle = -Double(pose.rotation) * 180 / .pi
    let placement = "translate(\(number(pose.x)) \(number(18 - pose.y))) rotate(\(number(angle))) translate(\(number(-w / 2)) \(number(-h / 2)))"
    let opacity = " opacity=\"\(number(pose.opacity))\""
    if pose.shape == .capsule {
        return "<rect transform=\"\(placement)\" width=\"\(number(w))\" height=\"\(number(h))\" rx=\"\(number(pose.radius))\" fill=\"url(#paint)\"\(opacity)/>"
    }
    let inner = "translate(0 \(number(h))) scale(1 -1)"
    let stroke = "fill=\"none\" stroke=\"url(#paint)\" stroke-width=\"\(number(pose.lineWidth))\" stroke-linecap=\"round\" stroke-linejoin=\"round\""
    let d: String
    switch pose.shape {
    case .ellipse:
        return "<ellipse transform=\"\(placement)\" cx=\"\(number(w/2))\" cy=\"\(number(h/2))\" rx=\"\(number(max(0.1,w-pose.lineWidth)/2))\" ry=\"\(number(max(0.1,h-pose.lineWidth)/2))\" \(stroke)\(opacity)/>"
    case .question:
        d = "M \(number(w*0.1)) \(number(h*0.72)) C \(number(w*0.16)) \(number(h)), \(number(w*0.46)) \(number(h)), \(number(w*0.52)) \(number(h*0.96)) C \(number(w*0.78)) \(number(h*0.94)), \(number(w*0.9)) \(number(h*0.82)), \(number(w*0.88)) \(number(h*0.66)) C \(number(w*0.86)) \(number(h*0.5)), \(number(w*0.53)) \(number(h*0.54)), \(number(w*0.5)) \(number(h*0.38)) L \(number(w*0.5)) \(number(h*0.22))"
    case .hourglassLeft:
        d = "M 0 \(number(h)) C \(number(w*0.08)) \(number(h*0.78)), \(number(w*0.92)) \(number(h*0.62)), \(number(w)) \(number(h/2)) C \(number(w*0.92)) \(number(h*0.38)), \(number(w*0.08)) \(number(h*0.22)), 0 0"
    case .hourglassRight:
        d = "M \(number(w)) \(number(h)) C \(number(w*0.92)) \(number(h*0.78)), \(number(w*0.08)) \(number(h*0.62)), 0 \(number(h/2)) C \(number(w*0.08)) \(number(h*0.38)), \(number(w*0.92)) \(number(h*0.22)), \(number(w)) 0"
    case .capsule: fatalError("Handled above")
    }
    return "<g transform=\"\(placement)\"\(opacity)><path transform=\"\(inner)\" d=\"\(d)\" \(stroke)/></g>"
}

let scenes = [IndicatorScenes.listening] + IndicatorScenes.all.values.sorted { $0.id < $1.id }
var catalog: [[String: Any]] = []
for scene in scenes {
    catalog.append([
        "id": scene.id, "transitionSeconds": scene.transition,
        "loopSeconds": scene.duration,
        "tracks": scene.tracks.map { track in
            ["keyTimes": track.keyTimes.map { $0.doubleValue },
             "phaseSeconds": track.phase,
             "poses": track.poses.map(poseObject)] as [String: Any]
        }
    ])
    let shapes = scene.firstPoses.map(svgElement).filter { !$0.isEmpty }.joined(separator: "\n  ")
    let svg = """
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 28 18" width="112" height="72" role="img" aria-label="\(scene.id) indicator">
      <defs><linearGradient id="paint" x1="0" y1="0" x2="28" y2="18" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#fafafa"/><stop offset="0.5" stop-color="#d5d6d8"/><stop offset="1" stop-color="#8a8d92"/></linearGradient></defs>
      \(shapes)
    </svg>
    """
    try svg.write(to: svgDirectory.appendingPathComponent("\(scene.id).svg"), atomically: true, encoding: .utf8)
}
let fileGlyphs = [
    "file-closed": "M 7 2 L 21 2 L 21 12 L 17 16 L 7 16 L 7 2 Z M 17 16 L 17 12 L 21 12",
    "file-open": "M 4 4 L 24 4 L 22 13 L 12 13 L 9 16 L 4 16 Z M 5 13 L 20 13 L 22 11"
]
for (name, path) in fileGlyphs {
    let svg = """
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 28 18" width="112" height="72" role="img" aria-label="\(name) indicator">
      <defs><linearGradient id="paint" x1="0" y1="0" x2="28" y2="18" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#fafafa"/><stop offset="0.5" stop-color="#d5d6d8"/><stop offset="1" stop-color="#8a8d92"/></linearGradient></defs>
      <path transform="translate(0 18) scale(1 -1)" d="\(path)" fill="none" stroke="url(#paint)" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>
    </svg>
    """
    try svg.write(to: svgDirectory.appendingPathComponent("\(name).svg"), atomically: true, encoding: .utf8)
}
let data = try JSONSerialization.data(withJSONObject: ["formatVersion": 1, "canvas": ["width": 28, "height": 18], "scenes": catalog], options: [.prettyPrinted, .sortedKeys])
try data.write(to: root.appendingPathComponent("scene_catalog.json"))
print("Exported \(scenes.count) scenes")
