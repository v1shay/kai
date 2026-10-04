import AppKit
import ApplicationServices

/// A snapshot taken before Kai takes focus; independent of dictation and navigation.
struct ApplicationContext {
    let text: String
    let imagePath: String?

    static func capture(lightweight: Bool) -> ApplicationContext {
        guard let app = NSWorkspace.shared.frontmostApplication else {
            return ApplicationContext(text: "Frontmost application unavailable", imagePath: nil)
        }
        let element = AXUIElementCreateApplication(app.processIdentifier)
        func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
            var value: CFTypeRef?
            guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
            return value
        }
        func child(_ name: String) -> AXUIElement? {
            guard let value = attribute(element, name), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
            return (value as! AXUIElement)
        }
        var lines = ["Application: \(app.localizedName ?? "Unknown")", "Bundle: \(app.bundleIdentifier ?? "Unknown")"]
        if let window = child(kAXFocusedWindowAttribute), let title = attribute(window, kAXTitleAttribute) as? String { lines.append("Window: \(title)") }
        var selected = ""
        if let focused = child(kAXFocusedUIElementAttribute) {
            for (name, key) in [("Role", kAXRoleAttribute), ("Description", kAXDescriptionAttribute), ("Selected text", kAXSelectedTextAttribute), ("Value", kAXValueAttribute)] {
                if let value = attribute(focused, key) as? String, !value.isEmpty {
                    lines.append("\(name): \(value.prefix(4000))")
                    if key == kAXSelectedTextAttribute { selected = value }
                }
            }
        }
        var path: String?
        if !lightweight || selected.isEmpty {
            if let image = CGWindowListCreateImage(.null, .optionOnScreenOnly, kCGNullWindowID, [.bestResolution]),
               let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) {
                let url = FileManager.default.temporaryDirectory.appendingPathComponent("kai-context-\(UUID().uuidString).png")
                if (try? data.write(to: url, options: .atomic)) != nil { path = url.path }
            }
            if path == nil { lines.append("Screenshot unavailable; screen recording permission may be required.") }
        }
        return ApplicationContext(text: lines.joined(separator: "\n"), imagePath: path)
    }
}
