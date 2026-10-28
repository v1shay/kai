import XCTest
@testable import NotchPrototype

final class UnifiedComposerStateTests:XCTestCase {
    func testOwnerReadinessOverridesHistoricalViewportWithoutChangingLegacyReadOnly()throws{
        var value:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadId":"code:chat","threadName":"Chat","history":[],"readOnly":true,"composerBusy":false,"approval":false,"activity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"","attachments":[]]
        func state()throws->KaiState{try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:value))}
        XCTAssertFalse(try state().inputBlocked,"A history viewport must still allow the owner's captured-send path")
        value["composerBusy"]=true;XCTAssertTrue(try state().inputBlocked)
        value.removeValue(forKey:"composerBusy");XCTAssertTrue(try state().inputBlocked,"Legacy read-only sources must remain read-only")
    }
}
