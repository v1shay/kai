import AppKit

@MainActor enum DictationComposerEditing {
    static func update(_ field:NSTextField,text:String){
        field.stringValue=text
        followEnd(field)
        // Field-editor layout can follow the stringValue update on the next
        // run-loop pass. Do not apply a stale dictation result after an edit.
        DispatchQueue.main.async { [weak field] in
            guard let field,field.stringValue == text else{return}
            followEnd(field)
        }
    }
    static func followEnd(_ field:NSTextField){
        guard let editor=field.currentEditor() as? NSTextView else{return}
        let end=NSRange(location:(editor.string as NSString).length,length:0)
        editor.setSelectedRange(end)
        if let container=editor.textContainer{editor.layoutManager?.ensureLayout(for:container)}
        editor.scrollRangeToVisible(end)
    }
}
