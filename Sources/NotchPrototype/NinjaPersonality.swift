import AppKit

/// Local phrase bags survive relaunches; no model calls or repeating greeting timer.
enum NinjaQuips {
    private static let transientPhrases:NSCache<NSString,NSArray>={let cache=NSCache<NSString,NSArray>();cache.countLimit=32;return cache}()
    enum Moment:String {case arrival,work,done,nearby}
    static func pool(_ moment:Moment,hour:Int,weekday:Int)->[String]{
        switch moment {
        case .arrival:
            let anytime=[
                "What can {name} do for you today?", "{name}, at your service. Mostly.",
                "Another day, another side quest.", "What's the mission?", "Got a little trouble for me?",
                "Your resident shadow has arrived.", "Boots on. Brain on. Your move.", "A fresh page. Suspicious potential.",
                "What are we making happen?", "Let's turn that idea into a thing.", "Quiet entrance. Big plans.",
                "{name} reporting for tiny adventures.", "One ninja. Many possibilities.", "Pick a quest. I'll bring the legs.",
                "What's on your mind, boss?", "Small ninja. Unreasonable ambition.", "Ready when your idea is.",
                "The dojo is open.", "What needs a little ninja energy?", "Shall we cause some progress?",
                "A wild {name} appears.", "You think it. I'll get moving.", "No cape. Still available.",
                "A little less talk. A little more done.", "Today's plot twist is yours to choose.",
                "What's first on our very unofficial agenda?", "Point me at the interesting part.",
                "I brought focus. You bring the plot.", "Tiny footsteps. Proper follow-through.",
                "Your next good idea has backup.", "Consider this a very small dramatic entrance.",
                "Let's make future-you proud.", "What are we untangling?", "The tiles and I have been expecting you.",
                "A blank canvas? Dangerous in our hands.", "{name} has entered the chat. Quietly.",
                "Let's give that to-do list a plot arc.", "Tools ready. Snacks unconfirmed.",
                "Something clever is about to happen.", "Same dojo. New possibilities."
            ]
            let timed:[String]
            switch hour {
            case 5..<12: timed=["Top of the morning?", "Morning, boss. What's the first quest?", "Coffee for you. Stealth for me.", "Fresh daylight. Fresh mischief.", "The early ninja gets the task.", "Good morning. Let's start with one good thing.", "Sun's up. Sleeves rolled.", "A little morning momentum?", "Today's still got that new-day smell.", "{name}'s morning shift starts now."]
            case 12..<17: timed=["Afternoon side quest?", "Let's give the afternoon a little purpose.", "Lunch done. Legends pending.", "A productive little plot twist?", "The day's not done with us yet.", "Midday mission control, {name} speaking.", "An afternoon well spent starts somewhere.", "Enough daylight for a little magic.", "Second wind, tiny footsteps.", "What shall we knock out before evening?"]
            case 17..<22: timed=["Evening, boss. What's cooking?", "Golden hour. Good ideas.", "Sunset side quest?", "The evening dojo is open.", "Let's tie up a loose end or two.", "A little progress before we call it a day?", "{name} on the evening watch.", "The day's final boss awaits.", "Evenings are for the interesting ideas.", "What would make tonight feel like a win?"]
            default: timed=["Night shift? I brought quiet footsteps.", "The moon gets it.", "Late-night idea? Let's hear it.", "After-hours dojo, open for business.", "Just us and a suspiciously good idea.", "A little midnight momentum?", "{name}, reporting from the quiet hours.", "The stars are out. What's our mission?", "One little quest before sleep?", "Soft lights. Sharp focus."]
            }
            return anytime+timed+([1,7].contains(weekday) ? ["Weekend side quest?", "No commute to the dojo today.", "A little weekend magic?"] : [])
        case .work:return ["On it. Tiny feet, big intent.","Consider the quest accepted.","Let me cook.","Into the interesting bits.","Rolling up my very small sleeves.","A little less wishing. A little more doing.","{name} has a plan.","Time to earn the headband.","Taking the scenic route through the details.","Eyes on the task.","Quietly making things happen.","Tools out. Focus in.","Let's give this some proper attention.","Small steps. Real progress.","Going full dojo on this one.","Off to investigate."]
        case .done:return ["That's a wrap. Tiny bow.","Quest complete. Your move.","One less thing on your list.","All tied up, neatly.","{name} sticks the landing.","The details have been dealt with.","Back from the side quest.","Done. I'll take a small victory lap.","A little progress, delivered.","Work down. Headband still on.","Finished, with a flourish.","Over to you, boss.","Another one for the good pile.","Mission complete. Bow optional.","Your results have arrived.","A satisfying little finish."]
        case .nearby:return ["Need a hand? I've got two tiny ones.","You rang, metaphorically?","Careful. Highly concentrated ninja.","Just checking the perimeter.","Yes, these are my real footsteps.","I'm small, not invisible.","The tiles are load-bearing. Probably.","Personal space? In this economy?","{name} sees that cursor.","A little wave for the audience.","Still here. Still ninja.","Something on your mind?","That pointer has main-character energy.","You handle the ideas. I'll handle the footwork.","No autographs. Just excellent footwork.","Mind the tiny dojo."]
        }
    }
    static func next(_ moment:Moment,name:String,date:Date=Date(),defaults:UserDefaults = .standard,persistChanges:Bool=NativePreferencePersistence.enabled)->String{
        let calendar=Calendar.current
        let choices=pool(moment,hour:calendar.component(.hour,from:date),weekday:calendar.component(.weekday,from:date))
        let key="ninja.phrases.v2."+moment.rawValue
        let transientKey="\(ObjectIdentifier(defaults)):\(key)" as NSString
        let recent=persistChanges ? (defaults.stringArray(forKey:key) ?? []):(transientPhrases.object(forKey:transientKey) as? [String] ?? defaults.stringArray(forKey:key) ?? [])
        let candidates=choices.filter{!recent.contains($0)}
        let picked=(candidates.isEmpty ? choices.filter{$0 != recent.last}:candidates).randomElement() ?? choices[0]
        // Keep a full cycle for the small pools and a long cross-day history for greetings.
        let history=Array((recent+[picked]).suffix(min(44,choices.count-1)))
        if persistChanges{defaults.set(history,forKey:key)}else{transientPhrases.setObject(history as NSArray,forKey:transientKey)}
        let clean=String(name.components(separatedBy:.newlines).joined(separator:" ").prefix(24))
        return picked.replacingOccurrences(of:"{name}",with:clean.isEmpty ? "your ninja":clean).lowercased()
    }
}

/// Small articulated pixel sprites, cached per palette. Actual limb poses rather
/// than scaling or sliding a rigid logo; Core Animation plays the frames.
enum NinjaPixelActor {
    enum Action: String, Sendable { case rest, walk, run, wave, inspect, leap, bow, climb, tuck }
    struct Key: Hashable, Sendable {
        let style: String
        let action: Action
        let step: Int
        let left: Bool
        init(style: String, action: Action, step: Int, left: Bool = false) {
            self.style = style; self.action = action; self.step = step % 8; self.left = left
        }
    }

    @MainActor private static var palettes: [String: NinjaActorRaster.Palette] = [:]
    @MainActor private static var paletteOrder: [String] = []
    @MainActor static var diagnosticPaletteCount: Int { palettes.count }

    /// Resolve the original AppKit colors on main. The worker receives immutable CGColors.
    @MainActor static func palette(style: String) -> NinjaActorRaster.Palette {
        if let cached = palettes[style] {
            paletteOrder.removeAll { $0 == style }; paletteOrder.append(style)
            return cached
        }
        let accent = NinjaAtmosphereView.color(style)
        let suit = style == "earth" ? NSColor(srgbRed: 0.26, green: 0.28, blue: 0.31, alpha: 1) : accent
        let shadow = suit.blended(withFraction: 0.66, of: .black)!
        let rim = suit.blended(withFraction: 0.38, of: .white)!
        let gold = NSColor(srgbRed: 0.96, green: 0.68, blue: 0.25, alpha: 1)
        let ink = NSColor(srgbRed: 0.07, green: 0.06, blue: 0.07, alpha: 1)
        let face = NSColor(srgbRed: 1, green: 0.76, blue: 0.16, alpha: 1)
        let palette = NinjaActorRaster.Palette(
            suit: suit.cgColor, shadow: shadow.cgColor, rim: rim.cgColor,
            gold: gold.cgColor, ink: ink.cgColor, face: face.cgColor,
            inkStroke: ink.withAlphaComponent(0.8).cgColor,
            goldShade: gold.blended(withFraction: 0.23, of: .black)!.cgColor,
            rimSeam: rim.withAlphaComponent(0.65).cgColor)
        if paletteOrder.count == 14 { palettes.removeValue(forKey: paletteOrder.removeFirst()) }
        palettes[style] = palette; paletteOrder.append(style)
        return palette
    }
}
