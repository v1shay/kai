import XCTest
@testable import NotchPrototype
final class PetSelectionPreferencesTests:XCTestCase {
    func testLastDisplayedPetSurvivesRelaunchAndMissingPetsFallBack(){
        let name="PetSelectionTests-"+UUID().uuidString,defaults=UserDefaults(suiteName:name)!
        defer{defaults.removePersistentDomain(forName:name)}
        let preferences=PetSelectionPreferences(defaults:defaults),ids=["codex","cat","dog"]
        preferences.selectedID="cat";preferences.lastUsedID="dog"
        XCTAssertEqual(PetSelectionPreferences(defaults:defaults).initialID(ids),"dog")
        preferences.lastUsedID="removed"
        XCTAssertEqual(preferences.initialID(ids),"cat")
        preferences.lastUsedID="codex"
        XCTAssertEqual(preferences.initialID(ids),"cat")
        preferences.selectedID="codex"
        XCTAssertEqual(preferences.initialID(ids),"codex")
    }
    func testExclusionCoversStartupAndRandomPoolButAllowsExplicitChoice(){
        let name="PetSelectionTests-"+UUID().uuidString,defaults=UserDefaults(suiteName:name)!
        defer{defaults.removePersistentDomain(forName:name)}
        let preferences=PetSelectionPreferences(defaults:defaults),ids=["codex","savage-codex-hacker","cat","dog"]
        XCTAssertEqual(preferences.automaticIDs(ids),["cat","dog"])
        XCTAssertEqual(preferences.initialID(ids),"cat")
        preferences.selectedID="savage-codex-hacker"
        XCTAssertEqual(preferences.initialID(ids),"savage-codex-hacker")
        preferences.excludeCodexPets=false
        XCTAssertEqual(PetSelectionPreferences(defaults:defaults).automaticIDs(ids),ids)
    }
}
