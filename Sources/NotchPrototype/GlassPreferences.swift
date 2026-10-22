import Foundation

struct GlassPreferences {
    enum Blend:Int,CaseIterable {case silk,airy,mist,broad
        var title:String {switch self{case .silk:return "Silk";case .airy:return "Airy";case .mist:return "Mist";case .broad:return "Broad feather"}}
        func strength(_ position:Double)->Double {
            let t=min(1,max(0,position))
            switch self {
            case .silk:return 1-t*t*(3-2*t)
            case .airy:return (1+cos(.pi*t))/2
            case .mist:return pow(1-t*t*(3-2*t),2)
            case .broad:return 1-pow(t,3)*(10-15*t+6*t*t)
            }
        }
    }
    static let controls:[(key:String,title:String,range:ClosedRange<Double>,initial:Double)]=[
        ("radius","Corner radius",0...48,18),("blurAmount","Blur amount",0...1,1),("opacity","Glass opacity",0...1,0.65),
        ("height","Fade height",0.08...0.85,0.38),("offset","Raise glass",0...0.25,0),
        ("haze","Frost haze",0...1,0.18),("tint","Pet tint",0...0.40,0.10),
        ("rim","Edge shimmer",0...0.25,0.035)]
    var radius=18.0,blurAmount=1.0,opacity=0.65,height=0.38,offset=0.0,haze=0.18,tint=0.10,rim=0.035
    var blur=1,blend=Blend.silk
    init(defaults:UserDefaults?=nil){
        guard let defaults else{return}
        for control in Self.controls {
            let raw=defaults.object(forKey:"glass."+control.key) as? Double ?? control.initial
            let value=raw.isFinite ? min(control.range.upperBound,max(control.range.lowerBound,raw)):control.initial
            switch control.key{case "radius":radius=value;case "blurAmount":blurAmount=value;case "opacity":opacity=value;case "height":height=value;case "offset":offset=value;case "haze":haze=value;case "tint":tint=value;case "rim":rim=value;default:break}
        }
        blur=min(2,max(0,defaults.integer(forKey:"glass.blur")))
        if defaults.object(forKey:"glass.blur") == nil{blur=1}
        blend=Blend(rawValue:defaults.integer(forKey:"glass.blend")) ?? .silk
    }
    static func reset(_ defaults:UserDefaults){for key in controls.map(\.key)+["blur","blend"]{defaults.removeObject(forKey:"glass."+key)}}
}
