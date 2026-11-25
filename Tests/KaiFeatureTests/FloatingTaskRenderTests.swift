import AppKit
import ImageIO
import XCTest
@testable import NotchPrototype

final class FloatingTaskRenderTests:XCTestCase {
    @MainActor func testNativePillLayoutAndPetPreview()throws{
        _=NSApplication.shared
        let root=NSView(frame:CGRect(x:0,y:0,width:260,height:260));root.wantsLayer=true;root.layer?.backgroundColor=NSColor(calibratedWhite:0.08,alpha:1).cgColor
        let assets=URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("kai_pets")
        let catalog=try XCTUnwrap(JSONSerialization.jsonObject(with:Data(contentsOf:assets.appendingPathComponent("animation_catalog.json"))) as? [String:Any])
        let pets=try XCTUnwrap(catalog["pets"] as? [[String:Any]]).filter{!PetSelectionPreferences.excludedIDs.contains($0["id"] as? String ?? "")}
        let profiles=try JSONDecoder().decode(GradientCatalog.self,from:Data(contentsOf:assets.appendingPathComponent("gradient_profiles.json")))
        for index in 0..<2{
            let pet=pets[index],id=try XCTUnwrap(pet["id"] as? String),sheet=try XCTUnwrap(pet["spritesheet"] as? String),grid=try XCTUnwrap(pet["grid"] as? [String:Any]),rows=try XCTUnwrap(grid["rows"] as? Int)
            let source=try XCTUnwrap(CGImageSourceCreateWithURL(assets.appendingPathComponent(sheet) as CFURL,nil)),image=try XCTUnwrap(CGImageSourceCreateImageAtIndex(source,0,nil))
            let y=CGFloat(142-index*112),row=FloatingTaskPetView(frame:CGRect(x:98,y:y+34,width:64,height:64)),pill=FloatingTaskStatusPill()
            pill.frame=CGRect(x:20,y:y,width:220,height:26);pill.label.stringValue=index == 0 ? "Updating the animation now":"swift test --filter FloatingTaskPetsTests"
            row.sprite.contents=image;row.sprite.contentsRect=CGRect(x:0,y:1-1/CGFloat(rows),width:1.0/8,height:1/CGFloat(rows))
            if let profile=profiles.profileByPetID[id]{pill.icon.setProfile(profile,wave:false)}
            pill.icon.showScene(index == 0 ? "pencil":"terminal",looping:false)
            root.addSubview(row);root.addSubview(pill);root.layoutSubtreeIfNeeded()
            XCTAssertTrue(pill.bounds.contains(pill.label.frame));XCTAssertGreaterThan(pill.label.frame.minX,pill.icon.layer.frame.maxX)
            XCTAssertEqual(pill.blendingMode,.behindWindow);XCTAssertEqual(pill.layer?.cornerRadius,13)
        }
        if let destination=ProcessInfo.processInfo.environment["KAI_FLOATING_RENDER"]{
            let bitmap=try XCTUnwrap(root.bitmapImageRepForCachingDisplay(in:root.bounds));root.cacheDisplay(in:root.bounds,to:bitmap)
            try XCTUnwrap(bitmap.representation(using:.png,properties:[:])).write(to:URL(fileURLWithPath:destination))
        }
    }
}
