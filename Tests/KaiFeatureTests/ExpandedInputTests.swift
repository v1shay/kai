import AppKit
import XCTest
@testable import NotchPrototype

@MainActor private final class PasteEditor:NSTextView {
    override func paste(_ sender:Any?) {
        // Avoid changing the user's clipboard while exercising the editor path.
        insertText("pasted text",replacementRange:selectedRange())
    }
}

final class ExpandedInputTests:XCTestCase {
    func testClickPaddingPreservesDirectHitsAndPicksTheNearestNeighbor(){
        let frames=[CGRect(x:0,y:0,width:20,height:16),CGRect(x:24,y:0,width:20,height:16)]
        for x in 0..<44 where x<20 || x>=24 {
            XCTAssertEqual(NotchInputRegion.nearestTarget(at:CGPoint(x:x,y:8),frames:frames),x<20 ? 0:1)
        }
        XCTAssertEqual(NotchInputRegion.nearestTarget(at:CGPoint(x:21,y:8),frames:frames),0)
        XCTAssertEqual(NotchInputRegion.nearestTarget(at:CGPoint(x:23,y:8),frames:frames),1)
        XCTAssertEqual(NotchInputRegion.nearestTarget(at:CGPoint(x:10,y:20),frames:frames),0)
        XCTAssertNil(NotchInputRegion.nearestTarget(at:CGPoint(x:100,y:8),frames:frames))
        XCTAssertNil(NotchInputRegion.nearestTarget(at:CGPoint(x:-1,y:8),frames:frames,limits:Array(repeating:CGRect(x:0,y:0,width:44,height:30),count:2)))
        // Close/hide padding must never steal a direct hit on the normal row.
        XCTAssertEqual(NotchInputRegion.nearestTarget(at:CGPoint(x:19,y:8),frames:frames.reversed()),1)
    }
    @MainActor func testPaddedComposerTargetsKeepVoiceSendAndTypingDistinct(){
        let ui=MiniAppInterface();var composer=CGRect.zero
        ui.composerFrameChanged={frame,_ in composer=frame}
        ui.layout(in:CGRect(x:0,y:0,width:500,height:400))
        var actions=[String](),focused=0;ui.onVoice={actions.append("voice")};ui.onSend={actions.append("send")};ui.onFocusComposer={focused+=1}
        let voice=CGRect(x:composer.maxX-51,y:composer.minY+3,width:22,height:22)
        let send=CGRect(x:composer.maxX-27,y:composer.minY+1,width:26,height:26)
        XCTAssertTrue(ui.handleClick(CGPoint(x:voice.maxX+0.5,y:voice.midY)))
        XCTAssertTrue(ui.handleClick(CGPoint(x:send.minX-0.5,y:send.midY)))
        XCTAssertTrue(ui.handleClick(CGPoint(x:voice.midX,y:voice.maxY+3)))
        XCTAssertEqual(actions,["voice","send","voice"])
        _=ui.handleClick(CGPoint(x:composer.minX+10,y:composer.midY))
        XCTAssertEqual(actions.count,3)
        XCTAssertEqual(focused,1)
    }
    func testExpandedShortcutsKeepEveryConfiguredActionAndOptionalDictationIsLocal()throws {
        let suite="KaiExpandedInput-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let preferences=ShortcutPreferences(defaults:defaults)
        XCTAssertFalse(preferences.expandedDictationOnly)
        for trigger in DictationTrigger.allCases {
            for action in ShortcutAction.allCases {
                preferences.set(action,for:trigger)
                XCTAssertEqual(preferences.action(for:trigger,expanded:true),action)
                XCTAssertEqual(preferences.action(for:trigger,expanded:false),action)
            }
        }
        preferences.expandedDictationOnly=true
        XCTAssertTrue(ShortcutPreferences(defaults:defaults).expandedDictationOnly)
        XCTAssertEqual(preferences.action(for:.command,expanded:true),.sendActiveChat)
        XCTAssertNil(preferences.action(for:.option,expanded:true))
        XCTAssertNil(preferences.action(for:.function,expanded:true))
        for trigger in DictationTrigger.allCases {
            XCTAssertEqual(preferences.action(for:trigger,expanded:false),.realtimeVoiceNewChatWithAppshot)
        }
        preferences.expandedDictationOnly=false
        XCTAssertEqual(preferences.action(for:.command,expanded:true),.realtimeVoiceNewChatWithAppshot)
    }

    @MainActor func testExpandedPanelPastesIntoFocusedEditorAndPreservesReplacementSelection()throws {
        _=NSApplication.shared
        let panel=NotchPanel(contentRect:CGRect(x:0,y:0,width:300,height:100),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        panel.isReleasedWhenClosed=false
        defer{panel.close()}
        let editor=PasteEditor(frame:panel.contentView!.bounds)
        panel.contentView!.addSubview(editor)
        editor.string="before selected after"
        XCTAssertTrue(panel.makeFirstResponder(editor))
        editor.setSelectedRange(NSRange(location:7,length:8))
        let paste=try XCTUnwrap(NSEvent.keyEvent(with:.keyDown,location:.zero,modifierFlags:.command,timestamp:0,windowNumber:panel.windowNumber,context:nil,characters:"v",charactersIgnoringModifiers:"v",isARepeat:false,keyCode:9))
        XCTAssertTrue(panel.performKeyEquivalent(with:paste))
        XCTAssertEqual(editor.string,"before pasted text after")
        let select=try XCTUnwrap(NSEvent.keyEvent(with:.keyDown,location:.zero,modifierFlags:.command,timestamp:0,windowNumber:panel.windowNumber,context:nil,characters:"a",charactersIgnoringModifiers:"a",isARepeat:false,keyCode:0))
        XCTAssertTrue(panel.performKeyEquivalent(with:select))
        XCTAssertEqual(editor.selectedRange(),NSRange(location:0,length:(editor.string as NSString).length))
    }
}
