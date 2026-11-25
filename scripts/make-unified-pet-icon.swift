import AppKit
import ImageIO
import UniformTypeIdentifiers

@main struct MakeUnifiedPetIcon {
    struct Catalog: Decodable {
        let pets: [Pet]
        struct Pet: Decodable {
            let id: String
            let spritesheet: String
            let grid: Grid
            let neutralFrame: Cell?
        }
        struct Grid: Decodable { let rows: Int }
        struct Cell: Decodable { let row: Int; let column: Int }
    }

    static func main() throws {
        let arguments = CommandLine.arguments
        guard arguments.count == 4 else { throw CocoaError(.fileWriteInvalidFileName) }
        let assets = URL(fileURLWithPath: arguments[1], isDirectory: true)
        let catalog = try JSONDecoder().decode(Catalog.self, from: Data(contentsOf: assets.appendingPathComponent("animation_catalog.json")))
        guard let pet = catalog.pets.first(where: { $0.id == arguments[2] }),
              let resource = CGImageSourceCreateWithURL(assets.appendingPathComponent(pet.spritesheet) as CFURL, nil),
              let sheet = CGImageSourceCreateImageAtIndex(resource, 0, nil) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        let iconset = URL(fileURLWithPath: arguments[3], isDirectory: true)
        try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
        let neutral = pet.neutralFrame ?? Catalog.Cell(row: 0, column: 0)
        for size in [16, 32, 128, 256, 512] {
            for scale in [1, 2] {
                let suffix = scale == 2 ? "@2x" : ""
                let file = iconset.appendingPathComponent("icon_\(size)x\(size)\(suffix).png")
                guard let frame = PetIconFramePipeline.frame(sheet: sheet, rows: pet.grid.rows, row: neutral.row, column: neutral.column, size: size * scale),
                      let output = CGImageDestinationCreateWithURL(file as CFURL, UTType.png.identifier as CFString, 1, nil) else {
                    throw CocoaError(.fileWriteUnknown)
                }
                CGImageDestinationAddImage(output, frame, nil)
                guard CGImageDestinationFinalize(output) else { throw CocoaError(.fileWriteUnknown) }
            }
        }
    }
}
