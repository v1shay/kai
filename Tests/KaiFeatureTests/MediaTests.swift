import XCTest
@testable import NotchPrototype

final class MediaTests:XCTestCase {
    func testStreamingPhrasesDoNotLosePartialWords() {
        let first=SpeechPlayback.splitPhrases("Hello. Here is a par",final:false)
        XCTAssertEqual(first.phrases,["Hello."]);XCTAssertEqual(first.remainder," Here is a par")
        let second=SpeechPlayback.splitPhrases(first.remainder+"tial sentence.",final:false)
        XCTAssertEqual(second.phrases,["Here is a partial sentence."]);XCTAssertEqual(second.remainder,"")
        XCTAssertEqual(SpeechPlayback.splitPhrases("Final words",final:true).phrases,["Final words"])
    }
    func testLongStreamingPhrasesStayBounded() {
        let split=SpeechPlayback.splitPhrases(String(repeating:"word ",count:100),final:true)
        XCTAssertTrue(split.phrases.allSatisfy{$0.count <= 180})
        XCTAssertEqual(split.phrases.joined(separator:" "),String(repeating:"word ",count:100).trimmingCharacters(in:.whitespaces))
    }
    func testYouTubeURLs() {
        for url in ["https://www.youtube.com/watch?v=dQw4w9WgXcQ", "youtube.com/shorts/dQw4w9WgXcQ", "https://youtu.be/dQw4w9WgXcQ?t=5", "https://www.youtube.com/live/dQw4w9WgXcQ"] {
            XCTAssertEqual(MediaCompanion.videoID(url),"dQw4w9WgXcQ")
        }
    }
    func testRejectsUnrelatedURLs() {
        for url in ["https://example.com/watch?v=dQw4w9WgXcQ", "https://youtube.com.evil.test/watch?v=dQw4w9WgXcQ", "https://youtube.com/shorts/invalid", "https://youtube.com/"] {
            XCTAssertNil(MediaCompanion.videoID(url))
        }
    }
    func testLegacyMessagesRemainDecodable() throws {
        let data=Data(#"{"id":"message","kind":"assistant","lines":[{"text":"Hello","style":""}]}"#.utf8)
        let message=try JSONDecoder().decode(KaiMessage.self,from:data)
        XCTAssertNil(message.speechText)
    }
    func testRawSpeechPreservesStreamingPrefix() throws {
        let data=Data(#"{"id":"message","kind":"assistant","speechText":"Hello there","lines":[{"text":"Hello there\n","style":""}]}"#.utf8)
        let message=try JSONDecoder().decode(KaiMessage.self,from:data)
        XCTAssertTrue(message.speechText!.hasPrefix("Hello"))
    }
}
