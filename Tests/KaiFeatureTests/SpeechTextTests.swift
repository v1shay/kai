import XCTest
@testable import NotchPrototype
final class SpeechTextTests:XCTestCase {
    func testReadsLabelsAndProseWithoutAddressesOrGeneratedMetadata(){
        let raw=#"Done. See [Google Slides](https://docs.google.com/presentation/d/long-id). :codex-file-citation{path="https://example.com/a" purpose="output"} [COMPLETE] Results are ready."#
        let speech=SpeechText.readable(raw)
        XCTAssertFalse(speech.contains("Google Slides"));XCTAssertTrue(speech.contains("Results are ready"))
        XCTAssertFalse(speech.contains("https"));XCTAssertFalse(speech.contains("codex"));XCTAssertFalse(speech.contains("COMPLETE"))
    }
    func testSkipsStatusChatterButKeepsCompletedResponse(){
        let text=SpeechText.readable("[STATUS] Checking files.\n\n[COMPLETE] The analysis supports this result.")
        XCTAssertFalse(text.contains("Checking files"))
        XCTAssertTrue(text.contains("The analysis supports this result"))
    }
    func testSkipsCodeAndFilePathsButKeepsInlineConcepts(){
        let text=SpeechText.readable("Use `cache` for speed.\n```swift\nprint(123)\n```\nSaved /Users/person/output.json")
        XCTAssertTrue(text.contains("cache"));XCTAssertFalse(text.contains("print"));XCTAssertFalse(text.contains("output.json"))
    }
    func testStreamingDoesNotReadPartialLink(){
        let split=SpeechPlayback.splitPhrases("See [the slides](https://docs.google.com/abc",final:false)
        XCTAssertTrue(split.phrases.isEmpty)
        XCTAssertEqual(split.remainder,"See [the slides](https://docs.google.com/abc")
        let complete=SpeechPlayback.splitPhrases(split.remainder+"). Ready.",final:true)
        XCTAssertFalse(complete.phrases.joined().contains("the slides"))
        XCTAssertFalse(complete.phrases.joined().contains("https"))
    }
}
