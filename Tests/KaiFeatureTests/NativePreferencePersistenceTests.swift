import AppKit
import XCTest
@testable import NotchPrototype

final class NativePreferencePersistenceTests:XCTestCase {
    func testSmokeModeSuppressesPersistenceWithoutChangingNormalLaunches(){
        XCTAssertTrue(NativePreferencePersistence.permitsWrites(arguments:["Kai"]))
        XCTAssertFalse(NativePreferencePersistence.permitsWrites(arguments:["Kai","--unified-smoke-test"]))
        XCTAssertTrue(NativePreferencePersistence.permitsWrites(arguments:["Kai","--unified-smoke-test-other"]))
    }

    func testPetMigrationAndPhraseHistoryRemainInMemoryInProbes()throws{
        let suite="KaiProbePreferences-"+UUID().uuidString,defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        defaults.set("cat",forKey:"explicitlySelectedPet");defaults.set("dog",forKey:"lastUsedPet")
        let before=defaults.persistentDomain(forName:suite)!
        let pets=PetSelectionPreferences(defaults:defaults,persistChanges:false)
        pets.selectedID="fox";pets.lastUsedID="fox";pets.excludeCodexPets=false
        XCTAssertEqual(pets.selectedID,"fox");XCTAssertEqual(pets.lastUsedID,"fox")
        XCTAssertFalse(pets.excludeCodexPets)
        pets.selectedID=nil;XCTAssertNil(pets.selectedID)
        let first=NinjaQuips.next(.work,name:"Zane",defaults:defaults,persistChanges:false)
        let second=NinjaQuips.next(.work,name:"Zane",defaults:defaults,persistChanges:false)
        XCTAssertNotEqual(first,second,"Transient phrase bags still avoid immediate repeats")
        XCTAssertEqual(NSDictionary(dictionary:before),NSDictionary(dictionary:defaults.persistentDomain(forName:suite)!))
    }

    @MainActor func testOwnerMenuVoiceAndSidebarUpdatesDoNotPersistProbeState()throws{
        _=NSApplication.shared
        let suite="KaiProbeMenuPreferences-"+UUID().uuidString,defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        defaults.set("current",forKey:"menuBar.identity");defaults.set("arbor",forKey:"realtimeVoice")
        defaults.set(["original":"/old"],forKey:"kaiChatProjects")
        let before=defaults.persistentDomain(forName:suite)!
        let menu=MenuBarIdentityMenu(defaults:defaults,persistChanges:false)
        menu.update(agent:"gemini");XCTAssertEqual(menu.mark,.provider("gemini"))
        let voice=RealtimeVoiceMenu(defaults:defaults,persistChanges:false)
        voice.synchronize("maple");XCTAssertEqual(voice.selection,.maple)
        let object:[String:Any]=["project":"/probe","projects":[["path":"/probe","name":"Probe"]],"chats":[["id":"probe-chat","name":"Probe chat","status":"idle"]],"standaloneChats":[],"standalone":false,"threadId":"probe-chat","threadName":"Probe chat","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[],"notice":"","attachments":[]]
        let state=try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
        let sidebar=MiniAppInterface(defaults:defaults,persistChanges:false)
        sidebar.layout(in:CGRect(x:0,y:0,width:450,height:320));sidebar.setState(state)
        let left=try XCTUnwrap(sidebar.layer.sublayers?.first)
        XCTAssertTrue(left.sublayers?.contains{$0.name=="Probe chat"} ?? false)
        sidebar.hideChat("probe-chat")
        XCTAssertFalse(left.sublayers?.contains{$0.name=="Probe chat"} ?? false)
        XCTAssertEqual(NSDictionary(dictionary:before),NSDictionary(dictionary:defaults.persistentDomain(forName:suite)!))
    }
}
