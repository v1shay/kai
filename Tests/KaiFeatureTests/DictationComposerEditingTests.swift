import XCTest
import AppKit
@testable import NotchPrototype
final class DictationComposerEditingTests:XCTestCase {
    @MainActor func testDictationKeepsCaretAtEndInsteadOfSelectingWholePrompt()throws {
        _=NSApplication.shared
        let window=NSWindow(contentRect:NSRect(x:0,y:0,width:300,height:100),styleMask:.borderless,backing:.buffered,defer:false)
        window.isReleasedWhenClosed=false
        let field=NSTextField(frame:NSRect(x:10,y:10,width:200,height:18))
        window.contentView?.addSubview(field);window.makeFirstResponder(field)
        let editor=try XCTUnwrap(field.currentEditor() as? NSTextView)
        editor.selectAll(nil)
        let text=(1...6).map{"Line \($0) with dictated words 🐙"}.joined(separator:"\n")
        DictationComposerEditing.update(field,text:text)
        XCTAssertEqual(field.stringValue,text)
        XCTAssertEqual(editor.selectedRange(),NSRange(location:(text as NSString).length,length:0))
        window.close()
    }
}
