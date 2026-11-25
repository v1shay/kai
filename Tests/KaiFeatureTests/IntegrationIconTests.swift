import XCTest
import AppKit
@testable import NotchPrototype
final class IntegrationIconTests:XCTestCase {
    @MainActor func testDownloadedSVGsRenderAndToolNamesResolve(){
        for id in IntegrationIcons.names where id != "linkedin" {
            let image=IntegrationIcons.image(id,color:.systemPink)
            XCTAssertNotNil(image,id)
            XCTAssertNotNil(image?.cgImage(forProposedRect:nil,context:nil,hints:nil),id)
        }
        XCTAssertEqual(IntegrationIcons.id(for:"mcp__codex_apps__google_drive__search"),"googledrive")
        XCTAssertEqual(IntegrationIcons.id(for:"https://docs.google.com/presentation/d/123"),"googleslides")
        XCTAssertEqual(IntegrationIcons.id(for:"mcp__notion__search"),"notion")
    }
}
