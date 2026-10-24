import AppKit
import XCTest
@testable import NotchPrototype

final class FloatingTaskPetsTests:XCTestCase {
    private func task(_ id:String,run:String="run",text:String="Working")->KaiActiveTask{KaiActiveTask(id:id,name:id,scene:"terminal",text:text,runId:run)}
    func testSnapshotRemovesMissingTasksAndKeepsRemainingTaskVisible(){
        var model=FloatingTaskLifecycle()
        XCTAssertTrue(model.update([task("a")],finished:[],enabled:true).isEmpty);XCTAssertTrue(model.tasks.isEmpty)
        _=model.update([task("a"),task("b")],finished:[],enabled:true)
        XCTAssertEqual(model.tasks.map(\.id),["a","b"])
        _=model.update([task("b")],finished:[],enabled:true)
        XCTAssertEqual(model.tasks.map(\.id),["b"])
        XCTAssertTrue(model.update([task("b")],finished:[KaiFinishedTask(id:"a",runId:"run")],enabled:true).isEmpty)
        XCTAssertEqual(model.tasks.map(\.id),["b"])
        XCTAssertEqual(model.update([],finished:[KaiFinishedTask(id:"b",runId:"run")],enabled:true).map(\.id),["b"])
        XCTAssertTrue(model.tasks.isEmpty)
    }
    func testLateCompletionCannotRemoveANewTurnAndActivityDoesNotReorderPets(){
        var model=FloatingTaskLifecycle()
        _=model.update([task("a"),task("b")],finished:[],enabled:true)
        _=model.update([task("b",text:"new command"),task("a",run:"next")],finished:[],enabled:true)
        XCTAssertEqual(model.tasks.map(\.id),["a","b"])
        XCTAssertTrue(model.update([task("a",run:"next"),task("b")],finished:[KaiFinishedTask(id:"a",runId:"run")],enabled:true).isEmpty)
        XCTAssertEqual(model.tasks.first?.runId,"next")
        _=model.update([],finished:[],enabled:false);XCTAssertTrue(model.tasks.isEmpty)
    }
    func testOffAndEmptySnapshotsRemovePetsWithoutCompletionEvents(){
        var model=FloatingTaskLifecycle()
        _=model.update([task("a"),task("b")],finished:[],enabled:true)
        let off=KaiActiveTask(id:"a",name:"a",scene:"off",text:"Ready",runId:"run")
        XCTAssertTrue(model.update([off,task("b")],finished:[],enabled:true).isEmpty)
        XCTAssertEqual(model.tasks.map(\.id),["b"])
        _=model.update([],finished:[],enabled:true)
        XCTAssertTrue(model.tasks.isEmpty)
    }
    func testResumedChatsRepairDuplicateAssignmentsWithoutChangingUniquePets(){
        let previous=["a":"cat","b":"cat","c":"dog","past":"bird"]
        let pets=TaskPetAssignments.reconcile(previous,taskIDs:["a","b","c"],available:["cat","dog","bird"],preferred:"cat",explicit:[])
        XCTAssertEqual(pets["a"],"cat");XCTAssertEqual(pets["c"],"dog");XCTAssertEqual(pets["b"],"bird")
        XCTAssertEqual(TaskPetAssignments.reconcile(pets,taskIDs:["c","b","a"],available:["cat","dog","bird"],preferred:"cat",explicit:[]),pets)
        XCTAssertEqual(pets["past"],"bird")
    }
    func testCyclingPrefersFreePetsAndManualChoiceMovesTheConflictingAssignment(){
        let previous=["a":"cat","b":"dog"]
        XCTAssertEqual(TaskPetAssignments.cycleChoices(for:"a",assignments:previous,taskIDs:["a","b"],available:["cat","dog","bird"]),["bird"])
        let chosen=["a":"dog","b":"dog"]
        let repaired=TaskPetAssignments.reconcile(chosen,taskIDs:["b","a"],available:["cat","dog"],preferred:"dog",explicit:["a","b"],priority:"a")
        XCTAssertEqual(repaired,["a":"dog","b":"cat"])
        let reserved=TaskPetAssignments.reconcile(["b":"cat"],taskIDs:["a","b"],available:["cat","dog"],preferred:"cat",explicit:[])
        XCTAssertEqual(reserved,["a":"dog","b":"cat"])
    }
    func testAssignmentsCanExceedPetPoolWithoutLosingTasks(){
        let pets=TaskPetAssignments.reconcile([:],taskIDs:["a","b","c"],available:["cat","dog"],preferred:"cat",explicit:[])
        XCTAssertEqual(pets.count,3);XCTAssertEqual(Set(pets.values),["cat","dog"])
    }
    func testAllCornersStayInsideVisibleScreenAndPetsStackWithoutOverlap(){
        let screen=CGRect(x:-1440,y:50,width:1440,height:850)
        for corner in FloatingTaskCorner.allCases{for text in [true,false]{for size:CGFloat in [40,64,112]{
            let first=corner.frames(screen:screen,index:0,size:size,text:text),second=corner.frames(screen:screen,index:1,size:size,text:text)
            XCTAssertTrue(screen.contains(first.pet));XCTAssertTrue(screen.contains(second.pet));XCTAssertFalse(first.pet.intersects(second.pet))
            XCTAssertEqual(first.pet.midX,second.pet.midX)
            if text{XCTAssertTrue(screen.contains(first.pill));XCTAssertEqual(first.pet.midX,first.pill.midX);XCTAssertLessThan(first.pill.maxY,first.pet.minY)}
        }}}
    }
    @MainActor func testDoubleClickOpensWithoutCyclingAndCancelledClicksDoNothing()async throws{
        let row=FloatingTaskPetView(frame:CGRect(x:0,y:0,width:64,height:64));var cycles=0,opens=0
        row.onSingle={cycles+=1};row.onDouble={opens+=1}
        row.handleClick(1);try await Task.sleep(nanoseconds:50_000_000);row.handleClick(2)
        try await Task.sleep(nanoseconds:UInt64((FloatingTaskPetView.clickDelay+0.05)*1_000_000_000))
        XCTAssertEqual(cycles,0);XCTAssertEqual(opens,1)
        row.handleClick(1)
        try await Task.sleep(nanoseconds:UInt64((FloatingTaskPetView.clickDelay+0.05)*1_000_000_000))
        XCTAssertEqual(cycles,1)
        row.handleClick(1);row.cancelClick()
        try await Task.sleep(nanoseconds:UInt64((FloatingTaskPetView.clickDelay+0.05)*1_000_000_000))
        XCTAssertEqual(cycles,1);XCTAssertEqual(opens,1)
    }
    @MainActor func testPreferencesPersistAndMalformedSizeDoesNotBreakMenu()throws{
        _=NSApplication.shared
        let suite="FloatingTaskPets-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite));defer{defaults.removePersistentDomain(forName:suite)}
        defaults.set(Double.nan,forKey:"floatingTasks.size")
        let controller=FloatingTaskPets(defaults:defaults)
        controller.menu.performActionForItem(at:0);XCTAssertTrue(defaults.bool(forKey:"floatingTasks.enabled"))
        controller.menu.performActionForItem(at:1);XCTAssertFalse(defaults.bool(forKey:"floatingTasks.text"))
        let corners=try XCTUnwrap(controller.menu.items.first{$0.title == "Screen corner"}?.submenu)
        corners.performActionForItem(at:3);XCTAssertEqual(defaults.string(forKey:"floatingTasks.corner"),"bottomLeft")
        let restored=FloatingTaskPets(defaults:defaults)
        XCTAssertTrue(restored.enabled);XCTAssertEqual(restored.menu.items[1].state,.off)
    }
}
