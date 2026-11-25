import XCTest
@testable import NotchPrototype

final class RunningChatSelectionTests:XCTestCase {
    private func state(selected:String?=nil,turn:String?=nil,focus:String?=nil,tasks:[String]=[])throws->KaiState {
        var obj:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadName":"Selected chat","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":tasks.map{["id":$0,"name":"Task \($0)","scene":"thinking","text":"Working"]},"notice":"","attachments":[]]
        if let selected{obj["threadId"]=selected};if let turn{obj["activeTurnId"]=turn};if let focus{obj["focusThreadId"]=focus}
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:obj))
    }
    func testExpansionChoosesFocusedRunningAgentOverIdleOrOtherSelectedChat()throws {
        XCTAssertEqual(RunningChatSelection.target(in:try state(selected:"idle",focus:"running",tasks:["running"]))?.id,"running")
        XCTAssertEqual(RunningChatSelection.target(in:try state(selected:"other",turn:"turn",focus:"running",tasks:["other","running"]))?.id,"running")
    }
    func testSelectionStaysOnAlreadyRunningChatAndFallsBackGracefully()throws {
        XCTAssertEqual(RunningChatSelection.target(in:try state(selected:"current",turn:"turn",tasks:["other"]))?.id,"current")
        XCTAssertEqual(RunningChatSelection.target(in:try state(focus:"finished",tasks:["running"]))?.id,"running")
        XCTAssertNil(RunningChatSelection.target(in:try state(selected:"idle")))
    }
    func testDotActivitiesNeverBecomeConversationTargets()throws {
        XCTAssertNil(RunningChatSelection.target(in:try state(focus:"dot:event",tasks:["dot:event"])))
        XCTAssertEqual(RunningChatSelection.target(in:try state(focus:"dot:event",tasks:["dot:event","running"]))?.id,"running")
    }
}
