import AppKit

/// Local phrase bags survive relaunches; no model calls or repeating greeting timer.
enum NinjaQuips {
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
    static func next(_ moment:Moment,name:String,date:Date=Date(),defaults:UserDefaults = .standard)->String{
        let calendar=Calendar.current
        let choices=pool(moment,hour:calendar.component(.hour,from:date),weekday:calendar.component(.weekday,from:date))
        let key="ninja.phrases.v2."+moment.rawValue
        let recent=defaults.stringArray(forKey:key) ?? []
        let candidates=choices.filter{!recent.contains($0)}
        let picked=(candidates.isEmpty ? choices.filter{$0 != recent.last}:candidates).randomElement() ?? choices[0]
        // Keep a full cycle for the small pools and a long cross-day history for greetings.
        defaults.set(Array((recent+[picked]).suffix(min(44,choices.count-1))),forKey:key)
        let clean=String(name.components(separatedBy:.newlines).joined(separator:" ").prefix(24))
        return picked.replacingOccurrences(of:"{name}",with:clean.isEmpty ? "your ninja":clean).lowercased()
    }
}

/// Small articulated pixel sprites, cached per palette. Actual limb poses rather
/// than scaling or sliding a rigid logo; Core Animation plays the frames.
@MainActor enum NinjaPixelActor {
    enum Action:String {case rest,walk,run,wave,inspect,leap,bow,climb,tuck}
    private static var cache=[String:CGImage]()
    @MainActor static func frame(style:String,action:Action,step:Int,left:Bool=false)->CGImage{
        let n=step%8,key="\(style)/\(action)/\(n)/\(left)"
        if let image=cache[key]{return image}
        let c=CGContext(data:nil,width:96,height:112,bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        c.scaleBy(x:2,y:2);c.translateBy(x:0,y:56);c.scaleBy(x:1,y:-1);c.setShouldAntialias(false)
        if left{c.translateBy(x:48,y:0);c.scaleBy(x:-1,y:1)}
        let color=NinjaAtmosphereView.color(style),dark=color.blended(withFraction:0.45,of:.black)!,light=color.blended(withFraction:0.25,of:.white)!
        let angle=Double(n)*Double.pi/4,swing=sin(angle)
        let moving=action == .walk || action == .run
        let bob=moving ? (n%4<2 ? 0.0:1.0):0
        let crouch=(action == .inspect || action == .tuck) ? (action == .tuck ? 4+sin(angle)*2:4.0):action == .bow ? Double([0,1,2,3,4,3,2,1][n]):0
        let lift=action == .leap ? Double([0,3,7,10,10,7,3,0][n]):0
        c.translateBy(x:0,y:bob+crouch-lift)
        func rect(_ x:Double,_ y:Double,_ w:Double,_ h:Double,_ fill:NSColor){c.setFillColor(fill.cgColor);c.fill(CGRect(x:x.rounded(),y:y.rounded(),width:w,height:h))}
        func limb(_ points:[CGPoint],_ fill:NSColor){
            for i in 1..<points.count{let a=points[i-1],b=points[i],steps=max(1,Int(hypot(b.x-a.x,b.y-a.y)));for s in 0...steps{let t=Double(s)/Double(steps);rect(Double(a.x)+Double(b.x-a.x)*t-2,Double(a.y)+Double(b.y-a.y)*t-2,4,4,fill)}}
        }
        let stride=action == .run ? 8.0:5.0
        for (i,x) in [20.0,28.0].enumerated(){
            let sign=i==0 ? 1.0:-1.0,dx=moving ? swing*stride*sign:action == .climb ? swing*3*sign:0
            let knee=CGPoint(x:x+dx*0.45+(action == .tuck ? sign*5:0),y:43-(moving ? max(0,-swing*sign)*3:action == .tuck ? 7:action == .climb ? max(0,swing*sign)*7:0))
            let foot=CGPoint(x:x+dx,y:49-(moving ? max(0,-swing*sign)*5:0)-(action == .inspect && i==1 ? 4:action == .tuck ? 9:action == .climb ? max(0,swing*sign)*9:0))
            limb([CGPoint(x:x,y:37),knee,foot],i==0 ? dark:color);rect(foot.x-2,foot.y,7,3,dark)
        }
        let arm = moving ? -swing*(action == .run ? 9:6):0
        if action != .climb{limb([CGPoint(x:17,y:29),CGPoint(x:13,y:34+arm*0.4),CGPoint(x:12+arm*0.5,y:39+arm)],dark)}
        rect(17,27,14,13,color);rect(17,36,14,3,dark);rect(23,36,3,3,light)
        // Scarf tail trails behind the direction of travel.
        rect(10,24,8,3,dark);rect(moving && n%4<2 ? 6:8,23,5,3,color)
        rect(16,10,16,3,color);rect(13,13,22,14,color);rect(16,26,16,3,dark)
        rect(15,18,18,7,dark);rect(17,20,5,2,light);rect(27,20,5,2,light)
        if action == .rest && n==7{rect(17,20,5,2,dark);rect(27,20,5,2,dark);rect(17,22,5,1,light);rect(27,22,5,1,light)}
        if action == .climb{
            limb([CGPoint(x:17,y:29),CGPoint(x:12,y:23),CGPoint(x:12,y:15+swing*6)],dark)
            limb([CGPoint(x:31,y:29),CGPoint(x:36,y:23),CGPoint(x:36,y:15-swing*6)],color)
        }else if action == .tuck{
            limb([CGPoint(x:31,y:29),CGPoint(x:34,y:34),CGPoint(x:29,y:38)],color)
        }else if action == .wave{
            limb([CGPoint(x:31,y:29),CGPoint(x:36,y:23),CGPoint(x:36+sin(angle)*3,y:14)],color)
        }else if action == .inspect{
            limb([CGPoint(x:31,y:29),CGPoint(x:35,y:35),CGPoint(x:38,y:39+sin(angle)*2)],light)
        }else if action == .leap{
            limb([CGPoint(x:31,y:29),CGPoint(x:36,y:23),CGPoint(x:37,y:17)],light)
        }else{
            limb([CGPoint(x:31,y:29),CGPoint(x:35,y:34-arm*0.4),CGPoint(x:35-arm*0.5,y:39-arm)],color)
        }
        let image=c.makeImage()!;cache[key]=image;return image
    }
}
