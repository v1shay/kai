import AppKit

enum IndicatorShape { case capsule, ellipse, question, hourglassLeft, hourglassRight }

struct IndicatorPose {
    var x, y, width, height, radius, rotation, opacity: CGFloat
    var shape: IndicatorShape = .capsule
    var lineWidth: CGFloat = 1.4
    static let hidden = IndicatorPose(x: 14, y: 9, width: 0.1, height: 0.1, radius: 0.05, rotation: 0, opacity: 0)
}

struct IndicatorTrack {
    let poses: [IndicatorPose], keyTimes: [NSNumber], phase: Double
    init(_ poses: [IndicatorPose], _ keyTimes: [NSNumber] = [0, 0.5, 1], phase: Double = 0) {
        self.poses = poses; self.keyTimes = keyTimes; self.phase = phase
    }
}

struct IndicatorScene {
    let id: String, transition: Double, duration: Double, tracks: [IndicatorTrack]
    var firstPoses: [IndicatorPose] {
        let visible = tracks.prefix(8).map { $0.poses[0] }
        return visible + Array(repeating:.hidden, count:8-visible.count)
    }
}

enum IndicatorScenes {
    private static func p(_ x: CGFloat,_ y: CGFloat,_ w: CGFloat,_ h: CGFloat,_ r: CGFloat = 1,_ rotation: CGFloat = 0,_ opacity: CGFloat = 1,_ shape: IndicatorShape = .capsule,_ lineWidth: CGFloat = 1.4) -> IndicatorPose {
        IndicatorPose(x: x, y: y, width: w, height: h, radius: r, rotation: rotation, opacity: opacity, shape: shape, lineWidth: lineWidth)
    }
    private static func still(_ pose: IndicatorPose) -> IndicatorTrack { IndicatorTrack([pose,pose,pose]) }
    private static func padded(_ tracks: [IndicatorTrack]) -> [IndicatorTrack] { tracks + Array(repeating: still(.hidden), count: max(0,8-tracks.count)) }
    private static func animate(_ pose: IndicatorPose, dx: CGFloat = 0, dy: CGFloat = 0, scale: CGFloat = 1, turn: CGFloat = 0, opacity: CGFloat? = nil, phase: Double = 0) -> IndicatorTrack {
        var mid = pose; mid.x += dx; mid.y += dy; mid.width *= scale; mid.height *= scale; mid.radius *= scale; mid.rotation += turn; if let opacity { mid.opacity = opacity }
        return IndicatorTrack([pose,mid,pose], phase: phase)
    }

    static let listening = IndicatorScene(id: "listening", transition: 0.62, duration: 0, tracks: padded(
        [3.5,10.5,17.5,24.5].map { still(p($0,9,3,7,1.5)) }
    ))

    static let thinking: IndicatorScene = {
        let sizes: [CGFloat] = [3.5,4,4.5,5]
        let dots = sizes.enumerated().map { i,size in
            IndicatorTrack([
                p(3.5+CGFloat(i)*7,9,size,size,size/2,0,0.42),
                p(3.5+CGFloat(i)*7,10.25,size*1.04,size*1.04,size/2,0,1),
                p(3.5+CGFloat(i)*7,9,size,size,size/2,0,0.42)
            ], phase: Double(i)*0.12)
        }
        return IndicatorScene(id: "thinking", transition: 0.9, duration: 1.4, tracks: padded(dots))
    }()

    static let pencil: IndicatorScene = {
        let angle: CGFloat = 0.62
        let base = [
            p(14,9,17,2.8,1.4,angle), p(14.5,9.55,12,0.7,0.35,angle,0.7),
            p(7.2,4.2,3.2,2.8,0.7,angle), p(21,13.8,3.4,2.8,0.45,angle),
            p(19,3.2,9,1.1,0.55,0,0.35)
        ]
        let tracks = base.enumerated().map { i,pose -> IndicatorTrack in
            var lifted = pose; if i < 4 { lifted.y += 0.7; lifted.rotation += 0.035 }
            if i == 4 { lifted.opacity = 1; lifted.width = 13 }
            return IndicatorTrack([pose,lifted,pose], phase: Double(i)*0.025)
        }
        return IndicatorScene(id: "pencil", transition: 0.68, duration: 1.25, tracks: padded(tracks))
    }()

    static let terminal: IndicatorScene = {
        let frame = [p(5,9,1.4,13,0.7),p(23,9,1.4,13,0.7),p(14,15.5,19,1.4,0.7),p(14,2.5,19,1.4,0.7)]
        let prompt = [p(10,10.4,6,1.4,0.7,0.55),p(10,7.6,6,1.4,0.7,-0.55)]
        let cursor = p(18,7,5,1.4,0.7)
        let tracks = frame.map(still) + prompt.map(still) + [IndicatorTrack([cursor,{ var x=cursor; x.opacity=0.15; return x }(),cursor])]
        return IndicatorScene(id: "terminal", transition: 0.68, duration: 1.05, tracks: padded(tracks))
    }()

    static let git: IndicatorScene = {
        let nodes = [p(6,4,4,4,2),p(6,14,4,4,2),p(22,9,4.5,4.5,2.25)]
        let stems = [p(6,9,1.5,7,0.75),p(14,9,15,1.5,0.75,0.31)]
        let tracks = nodes.enumerated().map { i,node in
            var pulse=node; pulse.width += 1.2; pulse.height += 1.2; pulse.radius += 0.6
            return IndicatorTrack([node,pulse,node], phase: Double(i)*0.18)
        } + stems.map(still) + [IndicatorTrack([p(7,7,1.8,1.8,0.9,0,0),p(14,9,2.4,2.4,1.2),p(21,11,1.8,1.8,0.9,0,0)])]
        return IndicatorScene(id: "git", transition: 0.72, duration: 1.55, tracks: padded(tracks))
    }()

    static let image: IndicatorScene = {
        let frame = [p(4,9,1.4,14,0.7),p(24,9,1.4,14,0.7),p(14,16,21,1.4,0.7),p(14,2,21,1.4,0.7)]
        let mountains = [p(9,7,9,1.6,0.8,0.72),p(15,8.3,10,1.6,0.8,-0.68),p(20.5,6.8,7,1.6,0.8,0.72)]
        let sun = p(19,12.5,3.4,3.4,1.7)
        let tracks = frame.map(still) + mountains.enumerated().map { i,mountain in
            var lift=mountain; lift.y += i == 1 ? 0.9 : 0.5
            return IndicatorTrack([mountain,lift,mountain], phase: Double(i)*0.08)
        } + [IndicatorTrack([sun,{ var glow=sun;glow.width=4.5;glow.height=4.5;glow.radius=2.25;return glow }(),sun])]
        return IndicatorScene(id: "image", transition: 0.74, duration: 1.8, tracks: tracks)
    }()

    static let document: IndicatorScene = {
        let frame = [p(6,9,1.4,14,0.7),p(22,8,1.4,12,0.7),p(14,16,17,1.4,0.7),p(14,2,17,1.4,0.7)]
        let fold = p(19.6,13.3,7,1.35,0.67,0.78)
        let lines = [p(13,10.5,10,1.35,0.67),p(14.5,7.5,13,1.35,0.67),p(11.5,4.8,7,1.35,0.67)]
        let tracks = frame.map(still) + [IndicatorTrack([fold,{ var turn=fold;turn.rotation=0.55;turn.opacity=0.62;return turn }(),fold])] + lines.enumerated().map { i,line in
            var written=line; written.width=2; written.x=line.x-line.width/2+1
            return IndicatorTrack([written,line,line], [0,0.58,1], phase: Double(i)*0.14)
        }
        return IndicatorScene(id: "document", transition: 0.74, duration: 1.85, tracks: tracks)
    }()

    static let code: IndicatorScene = {
        let bars = [p(7,11.5,7,1.7,0.85,-0.65),p(7,6.5,7,1.7,0.85,0.65),p(21,11.5,7,1.7,0.85,0.65),p(21,6.5,7,1.7,0.85,-0.65)]
        let slash = p(14,9,2,14,1,0.38)
        let tracks = bars.enumerated().map { i,bar in
            var breathe=bar; breathe.x += i < 2 ? -0.8 : 0.8; breathe.opacity=0.72
            return IndicatorTrack([bar,breathe,bar], phase: Double(i%2)*0.08)
        } + [IndicatorTrack([slash,{ var sweep=slash;sweep.rotation=0.55;sweep.height=15.5;return sweep }(),slash])]
        return IndicatorScene(id: "code", transition: 0.7, duration: 1.45, tracks: padded(tracks))
    }()

    static let search: IndicatorScene = {
        let globe = p(14,9,15,15,7.5,0,1,.ellipse,1.55)
        let meridian = p(14,9,4,14,2,0,0.9,.ellipse,1.2)
        var wide = meridian; wide.width = 11
        let equator = p(14,9,14,5,2.5,0,0.78,.ellipse,1.15)
        let dot = p(8.7,4.5,2.2,2.2,1.1)
        var far = dot; far.x=20.5; far.y=9; var top=dot; top.y=13.5
        let tracks = [still(globe),IndicatorTrack([meridian,wide,meridian]),still(equator),IndicatorTrack([dot,far,top,dot],[0,0.36,0.72,1])]
        return IndicatorScene(id:"search", transition:0.74, duration:1.75, tracks:padded(tracks))
    }()

    static let read: IndicatorScene = {
        let lenses = [p(8.5,9,8,7,3.5),p(19.5,9,8,7,3.5)], bridge = p(14,9.5,4,1.4,0.7)
        let arms = [p(3.7,11,5,1.3,0.65,-0.3),p(24.3,11,5,1.3,0.65,0.3)]
        let glints = [p(7.2,10.2,1.4,1.4,0.7,0,0.72),p(18.2,10.2,1.4,1.4,0.7,0,0.72)]
        let tracks = lenses.enumerated().map { animate($0.element, dy:$0.offset == 0 ? 0.45:-0.45, scale:1.05, phase:Double($0.offset)*0.12) } + [still(bridge)] + arms.map(still) + glints.enumerated().map { animate($0.element, dx:2.2, dy:-1.2, opacity:0.25, phase:Double($0.offset)*0.12) }
        return IndicatorScene(id:"read", transition:0.72, duration:1.65, tracks:padded(tracks))
    }()

    static let files: IndicatorScene = {
        let shell = [p(4,8,1.5,11,0.75),p(24,8,1.5,11,0.75),p(14,2.5,21,1.5,0.75),p(15.5,13.5,18,1.5,0.75),p(8,15.5,8,1.5,0.75)]
        let pages = [p(11,8,1.2,6,0.6),p(15,9,1.2,7,0.6),p(19,8,1.2,5,0.6)]
        let tracks = shell.map(still) + pages.enumerated().map { animate($0.element, dy:2, opacity:0.5, phase:Double($0.offset)*0.14) }
        return IndicatorScene(id:"files", transition:0.72, duration:1.7, tracks:tracks)
    }()

    static let settings: IndicatorScene = {
        let center=still(p(14,9,7,7,3.5,0,1,.ellipse,1.5))
        let teeth=(0..<6).map { i -> IndicatorTrack in
            let a=CGFloat(i) * .pi/3, tooth=p(14+cos(a)*7.3,9+sin(a)*7.3,4.4,1.7,0.85,a)
            return animate(tooth,scale:1.08,turn:0.08,phase:Double(i)*0.06)
        }
        return IndicatorScene(id:"settings",transition:0.68,duration:1.8,tracks:padded([center]+teeth))
    }()

    static let plan: IndicatorScene = {
        let checks = [p(5,12.5,4,1.4,0.7,0.65),p(7.2,13.5,6,1.4,0.7,-0.65),p(5,6.5,4,1.4,0.7,0.65),p(7.2,7.5,6,1.4,0.7,-0.65)]
        let lines = [p(18,13,12,1.5,0.75),p(18,9,12,1.5,0.75),p(18,5,12,1.5,0.75)]
        let marker = p(5,3.8,2.4,2.4,1.2)
        let tracks = checks.enumerated().map { animate($0.element, scale:1.08, opacity:0.65, phase:Double($0.offset/2)*0.16) } + lines.enumerated().map { animate($0.element, scale:0.72, opacity:0.58, phase:Double($0.offset)*0.13) } + [animate(marker, scale:1.3)]
        return IndicatorScene(id:"plan", transition:0.72, duration:1.8, tracks:tracks)
    }()

    static let review: IndicatorScene = {
        let lens = [p(10,12.5,8,1.4,0.7),p(10,5.5,8,1.4,0.7),p(6,9,1.4,7,0.7),p(14,9,1.4,7,0.7)]
        let handle = p(18.5,4.5,10,2,1,0.68), lines = [p(21,13,8,1.2,0.6),p(21,10,6,1.2,0.6)]
        let scan = p(10,9,1.2,5,0.6,0,0.55)
        let tracks = lens.map(still) + [animate(handle, turn:0.08)] + lines.map(still) + [animate(scan, dx:4, opacity:0.95)]
        return IndicatorScene(id:"review", transition:0.74, duration:1.6, tracks:tracks)
    }()

    static let permission: IndicatorScene = {
        let curve = p(14,10.5,10,11,5,0,1,.question,1.9), dot = p(14,3.5,2.4,2.4,1.2)
        let tracks = [animate(curve, scale:1.05, opacity:0.72),animate(dot, dy:-0.6, scale:1.18, phase:0.12)]
        return IndicatorScene(id:"permission", transition:0.72, duration:1.45, tracks:padded(tracks))
    }()

    static let question: IndicatorScene = {
        let bubble = [p(4,10,1.4,11,0.7),p(24,10,1.4,11,0.7),p(14,15.5,21,1.4,0.7),p(14,4.5,21,1.4,0.7),p(8,2.8,5,1.4,0.7,-0.55)]
        let dots = [p(9,10,2.6,2.6,1.3),p(14,10,2.6,2.6,1.3),p(19,10,2.6,2.6,1.3)]
        let tracks = bubble.map(still) + dots.enumerated().map { animate($0.element, dy:1.8, scale:1.12, opacity:0.55, phase:Double($0.offset)*0.16) }
        return IndicatorScene(id:"question", transition:0.74, duration:1.35, tracks:tracks)
    }()

    static let success: IndicatorScene = {
        let check = [p(8,6.5,9,2.2,1.1,-0.65),p(17,9,16,2.2,1.1,0.68)]
        let tracks = check.enumerated().map { animate($0.element, scale:1.06, opacity:0.78, phase:Double($0.offset)*0.08) }
        return IndicatorScene(id:"success", transition:0.62, duration:1.35, tracks:padded(tracks))
    }()

    static let failure: IndicatorScene = {
        let cross = [p(14,9,21,2.5,1.25,0.68),p(14,9,21,2.5,1.25,-0.68)]
        let tracks = cross.map { animate($0, scale:1.08, opacity:0.72) }
        return IndicatorScene(id:"failure", transition:0.58, duration:1.2, tracks:padded(tracks))
    }()

    static let declined: IndicatorScene = {
        let ring = p(14,9,16,16,8,0,1,.ellipse,1.6), slash = p(14,9,18,2.3,1.15,-0.62)
        let tracks = [animate(ring, scale:0.96, opacity:0.72),animate(slash, scale:1.04, turn:-0.08)]
        return IndicatorScene(id:"declined", transition:0.66, duration:1.45, tracks:padded(tracks))
    }()

    static let waiting: IndicatorScene = {
        let caps = [p(14,16,17,1.5,0.75),p(14,2,17,1.5,0.75)]
        let glass = [p(10,9,7,13,3.5,0,1,.hourglassLeft,1.45),p(18,9,7,13,3.5,0,1,.hourglassRight,1.45)]
        let grain = p(14,11,2,2,1), pile = p(14,4,8,1.5,0.75)
        let tracks = caps.map(still) + glass.map(still) + [animate(grain, dy:-6, scale:0.6),animate(pile, scale:1.18, opacity:0.7)]
        return IndicatorScene(id:"waiting", transition:0.72, duration:1.7, tracks:padded(tracks))
    }()

    static let tool: IndicatorScene = {
        let body = p(12,9,10,8,2), prongs = [p(19,11.5,7,1.8,0.9),p(19,6.5,7,1.8,0.9)]
        let cable = [p(5,9,7,1.8,0.9),p(2,12,1.8,7,0.9)]
        let contacts = [p(15,11.5,2,2,1),p(15,6.5,2,2,1)], pulse = p(4,9,2.5,2.5,1.25)
        let tracks = [animate(body, dx:1, scale:1.04)] + prongs.map(still) + cable.map(still) + contacts.enumerated().map { animate($0.element, scale:1.3, opacity:0.55, phase:Double($0.offset)*0.14) } + [animate(pulse, dx:9, opacity:0.3)]
        return IndicatorScene(id:"tool", transition:0.72, duration:1.45, tracks:tracks)
    }()

    static let agents: IndicatorScene = {
        let nodes = [p(5,9,4,4,2),p(20,14,4,4,2),p(20,4,4,4,2),p(14,9,3,3,1.5)]
        let links = [p(12,11.5,13,1.3,0.65,0.32),p(12,6.5,13,1.3,0.65,-0.32),p(20,9,1.3,7,0.65)]
        let tracks = nodes.enumerated().map { animate($0.element, scale:1.3, opacity:0.62, phase:Double($0.offset)*0.16) } + links.enumerated().map { animate($0.element, opacity:0.42, phase:Double($0.offset)*0.12) }
        return IndicatorScene(id:"agents", transition:0.76, duration:1.7, tracks:padded(tracks))
    }()

    static let compact: IndicatorScene = {
        let arrows = [p(6,12,7,1.5,0.75,0.62),p(6,6,7,1.5,0.75,-0.62),p(22,12,7,1.5,0.75,-0.62),p(22,6,7,1.5,0.75,0.62)]
        let stack = [p(14,11,8,1.6,0.8),p(14,7,8,1.6,0.8)]
        let tracks = arrows.enumerated().map { animate($0.element, dx:$0.offset < 2 ? 4:-4, dy:$0.offset%2 == 0 ? -2:2, scale:0.7, phase:Double($0.offset)*0.05) } + stack.enumerated().map { animate($0.element, scale:1.3, opacity:0.62, phase:Double($0.offset)*0.1) }
        return IndicatorScene(id:"compact", transition:0.7, duration:1.4, tracks:padded(tracks))
    }()

    static let warning: IndicatorScene = {
        let sides = [p(9,9,15,1.8,0.9,1.02),p(19,9,15,1.8,0.9,-1.02),p(14,3,16,1.8,0.9)]
        let mark = [p(14,10.5,2.2,6,1.1),p(14,5.4,2.4,2.4,1.2)]
        let tracks = sides.map(still) + mark.enumerated().map { animate($0.element, scale:1.18, opacity:0.55, phase:Double($0.offset)*0.1) }
        return IndicatorScene(id:"warning", transition:0.66, duration:1.25, tracks:padded(tracks))
    }()

    static let cycleIDs = ["thinking","pencil","terminal","git","image","document","code","search","read","files","plan","review","permission","question","success","failure","declined","waiting","tool","agents","compact","warning"]
    static let all = [thinking,pencil,terminal,git,image,document,code,search,read,files,settings,plan,review,permission,question,success,failure,declined,waiting,tool,agents,compact,warning].reduce(into: [String:IndicatorScene]()) { $0[$1.id] = $1 }
}
