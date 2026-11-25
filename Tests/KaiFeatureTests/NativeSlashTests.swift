import AppKit
import XCTest
@testable import NotchPrototype

final class NativeSlashTests:XCTestCase {
    func testInlineTokenUsesUtf16CaretAndPreservesSurroundingText(){
        let text="🦀 日本語 before /mod after",caret=("🦀 日本語 before /mod" as NSString).length
        let token=NativeSlashToken.at(text,selection:NSRange(location:caret,length:0))!
        XCTAssertEqual(token.query,"mod");XCTAssertEqual((text as NSString).substring(with:token.range),"/mod")
        for text in ["before /tmp/file","https://host/path","before word/model","before //comment","before /a.rs"]{XCTAssertNil(NativeSlashToken.at(text,selection:NSRange(location:(text as NSString).length,length:0)),text)}
        XCTAssertNil(NativeSlashToken.at("/model",selection:NSRange(location:0,length:6)))
    }
    @MainActor func testNativePickerNavigatesActualCommandsAndDismissesWithoutLosingDraft(){
        let picker=NativeSlashPicker(frame:.zero),commands=[KaiSlashCommand(name:"model",description:"Choose a model"),KaiSlashCommand(name:"motion",description:"Toggle motion")]
        let text="before /mo",selection=NSRange(location:text.utf16.count,length:0)
        picker.update(text:text,selection:selection,commands:commands,anchor:CGRect(x:10,y:10,width:300,height:18))
        XCTAssertTrue(picker.active);XCTAssertEqual(picker.matches.map(\.name),["model","motion"])
        var chosen="";picker.onAccept={chosen=$0};picker.move(1);picker.accept();XCTAssertEqual(chosen,"motion");XCTAssertFalse(picker.active)
        picker.update(text:text,selection:selection,commands:commands,anchor:.zero);picker.dismiss()
        picker.update(text:text,selection:selection,commands:commands,anchor:.zero);XCTAssertFalse(picker.active)
    }
}
