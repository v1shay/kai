import XCTest
@testable import NotchPrototype

final class SpeechIntegrationTests:XCTestCase {
    func testLocalVoicePlayback() async throws {
        guard let model=ProcessInfo.processInfo.environment["KAI_TEST_VOICE"] else{throw XCTSkip("Set KAI_TEST_VOICE to run local audio playback")}
        let ready=expectation(description:"voice loaded"),audio=expectation(description:"audio queued")
        let speech=await MainActor.run{SpeechPlayback()}
        await MainActor.run {
            speech.onStatus={ status in
                if status == "Local voice ready"{ready.fulfill()}
                if status.hasPrefix("Voice first audio:"){print(status);audio.fulfill()}
            }
            speech.configure(enabled:true,model:model)
        }
        await fulfillment(of:[ready],timeout:10)
        let initial=try state(text:nil,active:true)
        let streaming=try state(text:"Kai's local speech is ready.",active:true)
        await MainActor.run{speech.receive(initial);speech.receive(streaming)}
        await fulfillment(of:[audio],timeout:5)
        try await Task.sleep(nanoseconds:1_000_000_000)
        await MainActor.run{speech.stop()}
    }
    private func state(text:String?,active:Bool) throws->KaiState {
        var object:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadId":"test","threadName":"test","history":[],"readOnly":false,"approval":false,"activity":["scene":"thinking","text":"Thinking"],"activeTasks":[],"notice":"","attachments":[]]
        if active{object["activeTurnId"]="test-turn"}
        if let text{object["history"]=[["id":"response","kind":"assistant","speechText":text,"lines":[["text":text,"style":""]]]]}
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
    }
}
