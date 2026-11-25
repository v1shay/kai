import Foundation

/// Spoken prose skips link labels as well as addresses and generated
/// citation syntax, code blocks, or local file paths.
enum SpeechText {
    private static let protected=try! NSRegularExpression(pattern:#"\[STATUS\][\s\S]*?(?=\n\s*\n|\[COMPLETE\]|$)|```[\s\S]*?(?:```|$)|\[[^\]\n]*(?:\]\([^\)]*(?:\)|$)|$)|https?://[^\s<>]+|:?codex-[\w-]+\{[^}]*(?:\}|$)|</?[^>]*(?:>|$)|(?:/Users/|/private/|/tmp/|~/)[^\s]+"#)
    static func protectedRanges(_ text:String)->[NSRange]{protected.matches(in:text,range:NSRange(text.startIndex...,in:text)).map(\.range)}
    static func readable(_ source:String)->String {
        var text=source
        func replace(_ pattern:String,_ value:String){guard let regex=try? NSRegularExpression(pattern:pattern) else{return};text=regex.stringByReplacingMatches(in:text,range:NSRange(text.startIndex...,in:text),withTemplate:value)}
        replace(#"\[STATUS\][\s\S]*?(?=\n\s*\n|\[COMPLETE\]|$)"#," ")
        replace(#"(?m)^\s*(?:\[STATUS\]|(?:Used|Using) tool:|(?:Ran|Running) commands?[, ·:]).*$"#," ")
        replace(#"```[\s\S]*?(?:```|$)"#," ")
        replace(#"!?\[([^\]]*)\]\([^)]*(?:\)|$)"#," ")
        replace(#"https?://[^\s<>]+"#," ")
        replace(#":?codex-[\w-]+\{[^}]*(?:\}|$)"#," ")
        replace(#"(?:/Users/|/private/|/tmp/|~/)[^\s]+"#," ")
        replace(#"</?[^>]*(?:>|$)"#," ")
        replace(#"\[(?:STATUS|COMPLETE)\]"#," ")
        replace(#"(?m)^\s*(?:#{1,6}\s+|[-*+]\s+|>\s*)"#,"")
        replace(#"[`*_]"#,"")
        replace(#"\s+"#," ")
        return text.trimmingCharacters(in:.whitespacesAndNewlines)
    }
}
