import AppKit
import AVFoundation

final class FileGlyphLayer: CAShapeLayer {
    private let glyphSize:CGSize
    private let curve=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1)

    init(size:CGSize=CGSize(width:28,height:18)) {
        glyphSize=size;super.init();bounds=CGRect(origin:.zero,size:size);fillColor=nil;strokeColor=NSColor.white.cgColor;lineWidth=size.width/17.5;lineCap = .round;lineJoin = .round;setOpen(false,animated:false)
    }
    override init(layer:Any) { glyphSize=(layer as? FileGlyphLayer)?.glyphSize ?? CGSize(width:28,height:18);super.init(layer:layer) }
    required init?(coder:NSCoder) { fatalError() }

    func setOpen(_ open:Bool,animated:Bool) {
        let target=Self.path(open:open,size:glyphSize),from=presentation()?.path ?? path
        CATransaction.begin();CATransaction.setDisableActions(true);path=target;CATransaction.commit()
        guard animated else{return}
        let morph=CABasicAnimation(keyPath:"path");morph.fromValue=from;morph.toValue=target;morph.duration=0.42;morph.timingFunction=curve;add(morph,forKey:"fileOpen")
    }

    private static func path(open:Bool,size:CGSize)->CGPath {
        let p=CGMutablePath()
        if open { p.move(to:CGPoint(x:4,y:4));p.addLine(to:CGPoint(x:24,y:4));p.addLine(to:CGPoint(x:22,y:13));p.addLine(to:CGPoint(x:12,y:13));p.addLine(to:CGPoint(x:9,y:16));p.addLine(to:CGPoint(x:4,y:16));p.closeSubpath();p.move(to:CGPoint(x:5,y:13));p.addLine(to:CGPoint(x:20,y:13));p.addLine(to:CGPoint(x:22,y:11)) }
        else { p.move(to:CGPoint(x:7,y:2));p.addLine(to:CGPoint(x:21,y:2));p.addLine(to:CGPoint(x:21,y:12));p.addLine(to:CGPoint(x:17,y:16));p.addLine(to:CGPoint(x:7,y:16));p.addLine(to:CGPoint(x:7,y:2));p.closeSubpath();p.move(to:CGPoint(x:17,y:16));p.addLine(to:CGPoint(x:17,y:12));p.addLine(to:CGPoint(x:21,y:12)) }
        var transform=CGAffineTransform(scaleX:size.width/28,y:size.height/18);return p.copy(using:&transform) ?? p
    }
}

struct GradientCatalog: Decodable { let profileByPetID: [String: PetGradientProfile] }
struct PetGradientProfile: Decodable { let palette: PetPalette; let gradients: PetGradients }
struct PetPalette: Decodable { let shadow, primary, secondary, accent, highlight, foreground: String }
struct PetGradients: Decodable { let ambient, thinking, working, success, warning, error: GradientRecipe }
struct GradientRecipe: Decodable { let angleDegrees: Double, cycleDurationMs: Int, stops: [GradientStop] }
struct GradientStop: Decodable { let location: Double, color: String }

@MainActor final class NotchIndicatorEngine: NSObject {
    enum Mode: String { case off, listening, thinking, demo, scene, transitioning }
    private enum ThemeKind { case ambient, thinking, working, success, warning, error }

    let layer = CALayer(), fileLayer = CALayer(), accentLayer = CALayer(), leftAccentLayer = CALayer()
    private let content = CALayer(), maskLayer = CALayer(), base = CAGradientLayer(), shine = CAGradientLayer(), fileBase = CAGradientLayer(), fileShine = CAGradientLayer(), fileMask = FileGlyphLayer()
    private let softAccent=CALayer(), softLeftAccent=CALayer(), compactAccent=CALayer(), compactLeftAccent=CALayer(), accentMaskLayer=CAGradientLayer(), leftAccentMaskLayer=CAGradientLayer(), compactMaskLayer=CALayer(), compactLeftMaskLayer=CALayer()
    private let primitives = (0..<8).map { _ in CAShapeLayer() }, meter = VoiceMeter()
    private let centers: [CGFloat] = [3.5, 10.5, 17.5, 24.5]
    private var poses = IndicatorScenes.listening.firstPoses
    private var levels: [CGFloat] = [0.25, 0.3, 0.35, 0.4], profile: PetGradientProfile?
    private var timer: Timer?, demoWork: DispatchWorkItem?
    private var lastTick = ProcessInfo.processInfo.systemUptime, themeToken = 0, sceneToken = 0
    private var cycleToken = 0, currentSceneID: String?
    private var themeKind: ThemeKind = .ambient
    private var paletteTransitionLayers=[CALayer]()
    private var fileLoading=false
    private var showLeftAccent = false, showRightAccent = true
    private(set) var mode: Mode = .off

    override init() {
        super.init()
        layer.bounds = CGRect(x:0,y:0,width:28,height:18); layer.opacity=0; fileLayer.bounds=layer.bounds; fileLayer.opacity=0; accentLayer.opacity=0; leftAccentLayer.opacity=0
        for (mask,left) in [(accentMaskLayer,false),(leftAccentMaskLayer,true)] {
            mask.type = .radial; mask.colors=[NSColor.white.cgColor,NSColor.white.withAlphaComponent(0.5).cgColor,NSColor.white.withAlphaComponent(0.14).cgColor,NSColor.clear.cgColor]; mask.locations=[0,0.2,0.52,1]
            mask.startPoint=CGPoint(x:left ? 0.04:0.96,y:0.06); mask.endPoint=CGPoint(x:left ? 0.92:0.08,y:0.98)
        }
        compactMaskLayer.contents=glowMaskImage(); compactLeftMaskLayer.contents=glowMaskImage(mirrored:true)
        for mask in [compactMaskLayer,compactLeftMaskLayer] { mask.contentsGravity = .resizeAspectFill; mask.contentsScale=NSScreen.main?.backingScaleFactor ?? 2 }
        softAccent.mask=accentMaskLayer; softLeftAccent.mask=leftAccentMaskLayer; compactAccent.mask=compactMaskLayer; compactLeftAccent.mask=compactLeftMaskLayer
        accentLayer.addSublayer(softAccent); accentLayer.addSublayer(compactAccent); leftAccentLayer.addSublayer(softLeftAccent); leftAccentLayer.addSublayer(compactLeftAccent)
        softAccent.opacity=0; softLeftAccent.opacity=0
        layer.addSublayer(content); content.addSublayer(base); content.addSublayer(shine); content.mask = maskLayer
        fileLayer.addSublayer(fileBase); fileLayer.addSublayer(fileShine); fileLayer.mask=fileMask
        primitives.forEach { maskLayer.addSublayer($0) }
        for glow in [shine,fileShine] { glow.colors=[NSColor.clear.cgColor,NSColor.white.withAlphaComponent(0.72).cgColor,NSColor.clear.cgColor]; glow.startPoint=CGPoint(x:0,y:0.5); glow.endPoint=CGPoint(x:1,y:0.5) }
        meter.onLevels = { [weak self] values in self?.levels = values }
        layout(); setPoses(poses)
    }

    func layout() {
        content.frame=layer.bounds; maskLayer.frame=layer.bounds; base.frame=layer.bounds; shine.frame=layer.bounds
        fileBase.frame=fileLayer.bounds; fileShine.frame=fileLayer.bounds; fileMask.frame=fileLayer.bounds
        softAccent.frame=accentLayer.bounds; softLeftAccent.frame=leftAccentLayer.bounds; accentMaskLayer.frame=softAccent.bounds; leftAccentMaskLayer.frame=softLeftAccent.bounds
        compactAccent.frame=CGRect(x:accentLayer.bounds.width-68,y:0,width:68,height:39); compactLeftAccent.frame=CGRect(x:0,y:0,width:68,height:39); compactMaskLayer.frame=compactAccent.bounds; compactLeftMaskLayer.frame=compactLeftAccent.bounds
    }

    func setAccentVisibility(left: Bool, right: Bool) {
        showLeftAccent = left; showRightAccent = right; updateAccentOpacity()
    }

    func setAccentExpanded(_ expanded:Bool,duration:Double) {
        let compact:Float=expanded ? 0:1, soft:Float=expanded ? 1:0
        for (layer,target) in [(compactAccent,compact),(compactLeftAccent,compact),(softAccent,soft),(softLeftAccent,soft)] {
            let fade=CABasicAnimation(keyPath:"opacity"); fade.fromValue=layer.presentation()?.opacity ?? layer.opacity; fade.toValue=target; fade.duration=duration; fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1)
            CATransaction.begin(); CATransaction.setDisableActions(true); layer.opacity=target; CATransaction.commit(); if duration > 0 { layer.add(fade,forKey:"sizeBlend") }
        }
    }

    private func updateAccentOpacity() {
        // The corner gradients remain visible while the status icon is idle.
        leftAccentLayer.opacity = showLeftAccent ? 1:0; accentLayer.opacity = showRightAccent ? 1:0
    }

    func setProfile(_ newProfile: PetGradientProfile, wave: Bool) {
        profile = newProfile
        let recipe = recipe(for:newProfile, kind:themeKind)
        if wave && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion { waveTo(recipe, highlight:newProfile.palette.highlight, duration:0.52) }
        else { themeToken += 1;paletteTransitionLayers.forEach{$0.removeFromSuperlayer()};paletteTransitionLayers.removeAll();commit(recipe, to:base); commit(recipe,to:fileBase); commitAccent(recipe) }
        configureShine(newProfile.palette.highlight, duration: mode == .thinking ? 1.4 : 2.1)
    }

    func startListening(useMicrophone: Bool) {
        cancelCycle(); demoWork?.cancel(); themeKind = .ambient; useMicrophone ? requestMicrophone() : meter.stop(); layer.opacity = 1; updateAccentOpacity()
        if let profile { waveTo(profile.gradients.ambient, highlight: profile.palette.highlight, duration: 0.45) }
        transition(to: IndicatorScenes.listening, finalMode: .listening) { [weak self] in self?.startTimer() }
    }

    func setExternalLevels(_ values:[CGFloat]) { guard values.count>=4 else{return};levels=Array(values.prefix(4)) }

    func startDemo() {
        cancelCycle(); demoWork?.cancel(); themeKind = .ambient; meter.stop(); layer.opacity = 1; updateAccentOpacity()
        if let profile { waveTo(profile.gradients.ambient, highlight: profile.palette.highlight, duration: 0.45) }
        transition(to: IndicatorScenes.listening, finalMode: .demo) { [weak self] in
            guard let self else { return }; self.startTimer()
            let work = DispatchWorkItem { [weak self] in self?.transitionToThinking() }
            self.demoWork = work; DispatchQueue.main.asyncAfter(deadline: .now()+4.4, execute: work)
        }
    }

    func transitionToThinking() {
        cancelCycle(); demoWork?.cancel(); themeKind = .thinking; meter.stop(); layer.opacity = 1; updateAccentOpacity()
        if let profile { waveTo(profile.gradients.thinking, highlight: profile.palette.highlight, duration: 0.9) }
        transition(to: IndicatorScenes.thinking, finalMode: .thinking)
    }

    func showThinking() { transitionToThinking() }

    func showScene(_ id: String) {
        cancelCycle(); displayScene(id)
    }

    func startRandomCycle() {
        cycleToken += 1; let token = cycleToken
        var ids = IndicatorScenes.cycleIDs.shuffled()
        if ids.count > 1, ids.first == currentSceneID { ids.swapAt(0,1) }
        runCycle(ids, index: 0, token: token)
    }

    private func runCycle(_ ids: [String], index: Int, token: Int) {
        guard token == cycleToken, index < ids.count else { return }
        displayScene(ids[index])
        let delay = (IndicatorScenes.all[ids[index]]?.transition ?? 0.7) + 1.25
        DispatchQueue.main.asyncAfter(deadline: .now()+delay) { [weak self] in self?.runCycle(ids, index:index+1, token:token) }
    }

    private func cancelCycle() { cycleToken += 1 }

    private func displayScene(_ id: String) {
        guard let scene = IndicatorScenes.all[id] else { return }
        demoWork?.cancel(); currentSceneID = id
        themeKind = id == "thinking" || id == "waiting" ? .thinking : (id == "success" ? .success : (id == "warning" || id == "permission" || id == "question" ? .warning : (id == "failure" || id == "declined" ? .error : .working)))
        meter.stop(); layer.opacity = 1; updateAccentOpacity()
        if let profile { waveTo(recipe(for:profile, kind:themeKind), highlight:profile.palette.highlight, duration:scene.transition) }
        transition(to: scene, finalMode: .scene)
    }

    func hide() {
        cancelCycle(); sceneToken += 1; meter.stop(); demoWork?.cancel(); stopTimer(); primitives.forEach { $0.removeAllAnimations() }
        mode = .off; layer.opacity = 0; updateAccentOpacity()
    }

    private func requestMicrophone() {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: try? meter.start()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
                DispatchQueue.main.async { if granted { try? self?.meter.start() } }
            }
        default: break
        }
    }

    private func startTimer() {
        stopTimer(); lastTick = ProcessInfo.processInfo.systemUptime
        timer = Timer(timeInterval: 1/30, target: self, selector: #selector(timerFired), userInfo: nil, repeats: true)
        if let timer {RunLoop.main.add(timer,forMode:.common)}
    }

    private func stopTimer() { timer?.invalidate(); timer = nil }

    @objc private func timerFired() { tick() }

    private func tick() {
        let now = ProcessInfo.processInfo.systemUptime, dt = min(0.05, now-lastTick); lastTick = now
        switch mode {
        case .listening:
            updateListening(levels, dt: dt)
        case .demo:
            let syllable = max(0, sin(now*7.4))*max(0, sin(now*2.05+0.5)), amplitude = 0.1+0.78*syllable+0.11*pow(sin(now*13.1),2)
            var demo = [CGFloat]()
            for i in 0..<4 {
                let wave = sin(now*(10.7+Double(i)*0.73)+Double(i)*1.37)
                demo.append(CGFloat(amplitude)*(0.58+CGFloat(i)*0.045+0.28*CGFloat(wave*wave)))
            }
            updateListening(demo, dt: dt)
        default: break
        }
    }

    private func updateListening(_ values: [CGFloat], dt: Double) {
        let gains: [CGFloat] = [0.91,0.94,0.97,1]
        for i in 0..<4 {
            let target = 2.5+14.5*min(1, pow(max(0, values[i])*gains[i], 0.55)), rate: CGFloat = target > poses[i].height ? 22:9
            poses[i].height += (target-poses[i].height)*(1-exp(-CGFloat(dt)*rate)); poses[i].x = centers[i]; poses[i].y = 9
            poses[i].width = 3; poses[i].radius = 1.5; poses[i].rotation = 0; poses[i].opacity = 1
        }
        for i in 4..<8 { poses[i] = .hidden }
        setPoses(poses)
    }

    private func setPoses(_ next: [IndicatorPose]) {
        poses = next; CATransaction.begin(); CATransaction.setDisableActions(true)
        for i in 0..<8 {
            let pose = poses[i], primitive = primitives[i]
            primitive.bounds = CGRect(x: 0, y: 0, width: pose.width, height: pose.height); primitive.position = CGPoint(x: pose.x, y: pose.y)
            primitive.cornerRadius = pose.radius; primitive.transform = CATransform3DMakeRotation(pose.rotation, 0, 0, 1); primitive.opacity = Float(pose.opacity)
            if pose.shape == .capsule {
                primitive.backgroundColor = NSColor.white.cgColor; primitive.fillColor = nil; primitive.strokeColor = nil; primitive.path = nil
            } else {
                primitive.backgroundColor = nil; primitive.fillColor = nil; primitive.strokeColor = NSColor.white.cgColor
                primitive.lineWidth = pose.lineWidth; primitive.lineCap = .round; primitive.lineJoin = .round; primitive.path = shapePath(pose)
            }
        }
        CATransaction.commit()
    }

    private func shapePath(_ pose: IndicatorPose) -> CGPath {
        let w=pose.width, h=pose.height, inset=pose.lineWidth/2, path=CGMutablePath()
        switch pose.shape {
        case .ellipse: path.addEllipse(in:CGRect(x:inset,y:inset,width:max(0.1,w-pose.lineWidth),height:max(0.1,h-pose.lineWidth)))
        case .question:
            path.move(to:CGPoint(x:w*0.1,y:h*0.72)); path.addCurve(to:CGPoint(x:w*0.52,y:h*0.96),control1:CGPoint(x:w*0.16,y:h),control2:CGPoint(x:w*0.46,y:h))
            path.addCurve(to:CGPoint(x:w*0.88,y:h*0.66),control1:CGPoint(x:w*0.78,y:h*0.94),control2:CGPoint(x:w*0.9,y:h*0.82))
            path.addCurve(to:CGPoint(x:w*0.5,y:h*0.38),control1:CGPoint(x:w*0.86,y:h*0.5),control2:CGPoint(x:w*0.53,y:h*0.54)); path.addLine(to:CGPoint(x:w*0.5,y:h*0.22))
        case .hourglassLeft:
            path.move(to:CGPoint(x:0,y:h)); path.addCurve(to:CGPoint(x:w,y:h/2),control1:CGPoint(x:w*0.08,y:h*0.78),control2:CGPoint(x:w*0.92,y:h*0.62)); path.addCurve(to:CGPoint(x:0,y:0),control1:CGPoint(x:w*0.92,y:h*0.38),control2:CGPoint(x:w*0.08,y:h*0.22))
        case .hourglassRight:
            path.move(to:CGPoint(x:w,y:h)); path.addCurve(to:CGPoint(x:0,y:h/2),control1:CGPoint(x:w*0.92,y:h*0.78),control2:CGPoint(x:w*0.08,y:h*0.62)); path.addCurve(to:CGPoint(x:w,y:0),control1:CGPoint(x:w*0.08,y:h*0.38),control2:CGPoint(x:w*0.92,y:h*0.22))
        case .capsule: break
        }
        return path
    }

    func setFileOpen(_ open:Bool) { fileMask.setOpen(open,animated:true) }
    func setFileLoading(_ loading:Bool){
        guard fileLoading != loading else{return};fileLoading=loading
        fileShine.removeAnimation(forKey:"shine")
        if loading {
            let sweep=CABasicAnimation(keyPath:"locations");sweep.fromValue=[-0.35,-0.18,0];sweep.toValue=[1,1.18,1.35];sweep.duration=0.65;sweep.repeatCount = .infinity;sweep.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);fileShine.add(sweep,forKey:"fileLoading")
        } else {
            fileShine.removeAnimation(forKey:"fileLoading")
            if let profile {configureShine(profile.palette.highlight,duration:mode == .thinking ? 1.4:2.1)}
        }
    }

    private func capturePresentation() {
        for i in 0..<8 {
            if let p = primitives[i].presentation() {
                poses[i] = IndicatorPose(x: p.position.x, y: p.position.y, width: p.bounds.width, height: p.bounds.height,
                    radius: p.cornerRadius, rotation: atan2(p.transform.m12,p.transform.m11), opacity: CGFloat(p.opacity), shape:poses[i].shape, lineWidth:poses[i].lineWidth)
            }
            primitives[i].removeAllAnimations()
        }
        setPoses(poses)
    }

    private func transition(to scene: IndicatorScene, finalMode: Mode, completion: (() -> Void)? = nil) {
        sceneToken += 1; let token = sceneToken; stopTimer(); capturePresentation(); let from = poses, target = scene.firstPoses
        mode = .transitioning; setPoses(target)
        for i in 0..<8 {
            let group = CAAnimationGroup(), a = from[i], b = target[i]
            func basic(_ path: String,_ start: Any,_ end: Any) -> CABasicAnimation { let x=CABasicAnimation(keyPath:path);x.fromValue=start;x.toValue=end;return x }
            group.animations = [
                basic("position",NSValue(point:NSPoint(x:a.x,y:a.y)),NSValue(point:NSPoint(x:b.x,y:b.y))),
                basic("bounds",NSValue(rect:NSRect(x:0,y:0,width:a.width,height:a.height)),NSValue(rect:NSRect(x:0,y:0,width:b.width,height:b.height))),
                basic("cornerRadius",a.radius,b.radius), basic("transform.rotation.z",a.rotation,b.rotation), basic("opacity",a.opacity,b.opacity)
            ]
            group.duration = scene.transition; group.timingFunction = CAMediaTimingFunction(controlPoints: 0.2,0.72,0.24,1)
            primitives[i].add(group, forKey: "sceneTransition")
        }
        DispatchQueue.main.asyncAfter(deadline: .now()+scene.transition) { [weak self] in
            guard let self, self.sceneToken == token else { return }; self.mode = finalMode; self.installLoop(scene); completion?()
        }
    }

    private func installLoop(_ scene: IndicatorScene) {
        guard scene.duration > 0 else { return }; let now = CACurrentMediaTime()
        for (i,track) in scene.tracks.prefix(8).enumerated() {
            let group = CAAnimationGroup()
            func keyframes(_ path: String,_ values: [Any]) -> CAKeyframeAnimation { let x=CAKeyframeAnimation(keyPath:path);x.values=values;x.keyTimes=track.keyTimes;return x }
            group.animations = [
                keyframes("position",track.poses.map { NSValue(point:NSPoint(x:$0.x,y:$0.y)) }),
                keyframes("bounds",track.poses.map { NSValue(rect:NSRect(x:0,y:0,width:$0.width,height:$0.height)) }),
                keyframes("cornerRadius",track.poses.map(\.radius)), keyframes("transform.rotation.z",track.poses.map(\.rotation)), keyframes("opacity",track.poses.map(\.opacity))
            ]
            if track.poses.allSatisfy({ $0.shape != .capsule }) { group.animations?.append(keyframes("path",track.poses.map { shapePath($0) })) }
            group.duration = scene.duration; group.beginTime = now+track.phase; group.repeatCount = .infinity
            group.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut); primitives[i].add(group, forKey: "sceneLoop")
        }
    }

    private func commit(_ recipe: GradientRecipe, to gradient: CAGradientLayer) {
        let points = endpoints(recipe.angleDegrees); CATransaction.begin(); CATransaction.setDisableActions(true)
        gradient.colors = recipe.stops.map { color($0.color) }; gradient.locations = recipe.stops.map { NSNumber(value: $0.location) }
        gradient.startPoint = points.0; gradient.endPoint = points.1; CATransaction.commit()
    }

    private func recipe(for profile: PetGradientProfile, kind: ThemeKind) -> GradientRecipe {
        switch kind {
        case .ambient: profile.gradients.ambient
        case .thinking: profile.gradients.thinking
        case .working: profile.gradients.working
        case .success: profile.gradients.success
        case .warning: profile.gradients.warning
        case .error: profile.gradients.error
        }
    }

    private func glowMaskImage(mirrored:Bool=false) -> CGImage? {
        let scale=2,width=136,height=78,space=CGColorSpaceCreateDeviceRGB()
        guard let context=CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:width*4,space:space,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue), let gradient=CGGradient(colorsSpace:space,colors:[NSColor.white.cgColor,NSColor.white.withAlphaComponent(0.32).cgColor,NSColor.clear.cgColor] as CFArray,locations:[0,0.42,1]) else{return nil}
        context.scaleBy(x:CGFloat(scale),y:CGFloat(scale)*0.62); let center=CGPoint(x:mirrored ? 5:63,y:(39*0.22)/0.62)
        context.drawRadialGradient(gradient,startCenter:center,startRadius:0,endCenter:center,endRadius:66,options:[.drawsAfterEndLocation]); return context.makeImage()
    }

    private func accentColor(_ recipe:GradientRecipe,_ alpha:CGFloat) -> CGColor { color(recipe.stops[min(1,recipe.stops.count-1)].color,alpha) }

    private func huePath(from start: CGColor, to end: CGColor, steps: Int = 16) -> [CGColor] {
        func hsba(_ value:CGColor) -> (CGFloat,CGFloat,CGFloat,CGFloat) {
            let c=NSColor(cgColor:value)?.usingColorSpace(.sRGB) ?? .clear; var h:CGFloat=0,s:CGFloat=0,b:CGFloat=0,a:CGFloat=0
            c.getHue(&h,saturation:&s,brightness:&b,alpha:&a); return(h,s,b,a)
        }
        let a=hsba(start), b=hsba(end); var delta=b.0-a.0
        if delta > 0.5 { delta -= 1 }; if delta < -0.5 { delta += 1 }
        return (0..<steps).map { i in let t=CGFloat(i)/CGFloat(steps-1), h=(a.0+delta*t).truncatingRemainder(dividingBy:1)
            return NSColor(calibratedHue:h < 0 ? h+1:h,saturation:a.1+(b.1-a.1)*t,brightness:a.2+(b.2-a.2)*t,alpha:a.3+(b.3-a.3)*t).cgColor }
    }

    private func commitAccent(_ recipe: GradientRecipe) {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        for (layer,alpha) in [(compactAccent,0.58),(compactLeftAccent,0.58),(softAccent,0.48),(softLeftAccent,0.48)] { layer.removeAnimation(forKey:"colorFlow"); layer.backgroundColor=accentColor(recipe,alpha) }
        CATransaction.commit()
    }

    private func waveAccentTo(_ recipe: GradientRecipe, duration: Double, token: Int) {
        CATransaction.begin(); CATransaction.setDisableActions(true)
        for (layer,alpha) in [(compactAccent,0.58),(compactLeftAccent,0.58),(softAccent,0.48),(softLeftAccent,0.48)] {
            let target=accentColor(recipe,alpha)
            let start=layer.presentation()?.backgroundColor ?? layer.backgroundColor ?? target; layer.backgroundColor=target
            let flow=CAKeyframeAnimation(keyPath:"backgroundColor"); flow.values=huePath(from:start,to:target); flow.duration=max(0.68,duration)
            flow.calculationMode = .linear; flow.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1); layer.add(flow,forKey:"colorFlow")
        }
        CATransaction.commit()
    }

    private func waveTo(_ recipe: GradientRecipe, highlight: String, duration: Double) {
        paletteTransitionLayers.forEach{$0.removeFromSuperlayer()};paletteTransitionLayers.removeAll()
        themeToken += 1; let token = themeToken; waveAccentTo(recipe,duration:duration,token:token)
        let incoming = CAGradientLayer(), reveal = CAGradientLayer(), edge = CAGradientLayer(), fileIncoming=CAGradientLayer(), fileReveal=CAGradientLayer(), fileEdge=CAGradientLayer()
        incoming.frame = content.bounds; commit(recipe, to: incoming)
        reveal.frame = content.bounds; reveal.startPoint = CGPoint(x: 0, y: 0.5); reveal.endPoint = CGPoint(x: 1, y: 0.5)
        reveal.colors = [NSColor.white.cgColor,NSColor.white.cgColor,NSColor.clear.cgColor,NSColor.clear.cgColor]; reveal.locations = [-0.25,-0.1,0,0.15]
        incoming.mask = reveal; content.insertSublayer(incoming, above: base)
        edge.frame = content.bounds; edge.startPoint = CGPoint(x: 0, y: 0.5); edge.endPoint = CGPoint(x: 1, y: 0.5)
        edge.colors = [NSColor.clear.cgColor,color(highlight,0.9),NSColor.clear.cgColor]; edge.locations = [-0.15,0,0.15]; content.insertSublayer(edge, above: incoming)
        fileIncoming.frame=fileLayer.bounds; commit(recipe,to:fileIncoming); fileReveal.frame=fileLayer.bounds; fileReveal.startPoint=reveal.startPoint; fileReveal.endPoint=reveal.endPoint; fileReveal.colors=reveal.colors; fileReveal.locations=reveal.locations; fileIncoming.mask=fileReveal; fileLayer.insertSublayer(fileIncoming,above:fileBase)
        fileEdge.frame=fileLayer.bounds; fileEdge.startPoint=edge.startPoint; fileEdge.endPoint=edge.endPoint; fileEdge.colors=edge.colors; fileEdge.locations=edge.locations; fileLayer.insertSublayer(fileEdge,above:fileIncoming)
        paletteTransitionLayers=[incoming,edge,fileIncoming,fileEdge]
        let wipe = CABasicAnimation(keyPath: "locations"); wipe.fromValue = reveal.locations; wipe.toValue = [1,1.1,1.25,1.4]; wipe.duration = duration; wipe.timingFunction = CAMediaTimingFunction(controlPoints: 0.18,0.72,0.24,1)
        let streak = CABasicAnimation(keyPath: "locations"); streak.fromValue = edge.locations; streak.toValue = [0.85,1,1.15]; streak.duration = duration; streak.timingFunction = wipe.timingFunction
        reveal.locations=[1,1.1,1.25,1.4]; edge.locations=[0.85,1,1.15]; fileReveal.locations=reveal.locations; fileEdge.locations=edge.locations
        reveal.add(wipe,forKey:"reveal"); edge.add(streak,forKey:"edge"); fileReveal.add(wipe.copy() as! CAAnimation,forKey:"reveal"); fileEdge.add(streak.copy() as! CAAnimation,forKey:"edge")
        DispatchQueue.main.asyncAfter(deadline: .now()+duration) { [weak self, weak incoming, weak edge, weak fileIncoming, weak fileEdge] in
            guard let self, self.themeToken == token else { incoming?.removeFromSuperlayer(); edge?.removeFromSuperlayer(); fileIncoming?.removeFromSuperlayer(); fileEdge?.removeFromSuperlayer(); return }
            self.commit(recipe,to:self.base); self.commit(recipe,to:self.fileBase); incoming?.removeFromSuperlayer(); edge?.removeFromSuperlayer(); fileIncoming?.removeFromSuperlayer(); fileEdge?.removeFromSuperlayer()
        }
    }

    private func configureShine(_ hex: String, duration: Double) {
        shine.removeAllAnimations(); fileShine.removeAllAnimations(); shine.colors=[NSColor.clear.cgColor,color(hex,0.65),NSColor.clear.cgColor]; fileShine.colors=shine.colors; shine.locations=[-0.35,-0.18,0]; fileShine.locations=shine.locations
        let animation = CABasicAnimation(keyPath: "locations"); animation.fromValue = shine.locations; animation.toValue = [1,1.18,1.35]
        animation.duration=duration; animation.repeatCount = .infinity; animation.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut); shine.add(animation,forKey:"shine")
        let fileAnimation=animation.copy() as! CABasicAnimation;fileAnimation.duration=fileLoading ? 0.65:duration;fileShine.add(fileAnimation,forKey:fileLoading ? "fileLoading":"shine")
    }

    private func endpoints(_ degrees: Double) -> (CGPoint,CGPoint) {
        let radians = (degrees-90)*Double.pi/180, dx = cos(radians)/2, dy = sin(radians)/2
        return (CGPoint(x: 0.5-dx, y: 0.5-dy), CGPoint(x: 0.5+dx, y: 0.5+dy))
    }

    private func color(_ hex: String, _ alpha: CGFloat = 1) -> CGColor {
        let value = UInt64(hex.dropFirst(), radix: 16) ?? 0
        return NSColor(srgbRed: CGFloat((value>>16)&255)/255, green: CGFloat((value>>8)&255)/255, blue: CGFloat(value&255)/255, alpha: alpha).cgColor
    }
}

private final class VoiceMeter {
    let engine = AVAudioEngine(); var onLevels: (([CGFloat])->Void)?
    private var installed = false, noiseFloor: Float = 0.004, peak: Float = 0.04

    func start() throws {
        stop(); let input = engine.inputNode, format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 else { return }
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer,_ in
            guard let self, let samples = buffer.floatChannelData?[0] else { return }
            let count = Int(buffer.frameLength), rate = Float(format.sampleRate)
            let bands: [[Float]] = [[140,220],[320,520],[750,1100],[1600,2600]]
            var squareSum: Float = 0
            for i in 0..<count { squareSum += samples[i]*samples[i] }
            let rms = sqrt(squareSum/Float(max(count,1)))
            if rms < self.noiseFloor*1.5 { self.noiseFloor = self.noiseFloor*0.97+rms*0.03 }
            else { self.noiseFloor = self.noiseFloor*0.999+rms*0.001 }
            self.peak = max(rms, self.peak*0.985)
            let signal = max(0, rms-self.noiseFloor), fixed = min(1, signal*18)
            let adaptive = min(1, signal/max(0.018, self.peak-self.noiseFloor))*0.68, voice = max(fixed, adaptive)
            let values = bands.map { frequencies -> CGFloat in
                var tonal: Float = 0
                for frequency in frequencies {
                    let coefficient = 2*cos(2*Float.pi*frequency/rate); var previous: Float = 0, previous2: Float = 0
                    for i in 0..<count { let current = samples[i]+coefficient*previous-previous2; previous2 = previous; previous = current }
                    let power = max(0, previous2*previous2+previous*previous-coefficient*previous*previous2)
                    tonal = max(tonal, sqrt(power)/Float(max(count,1)))
                }
                let bandPresence = min(1, tonal/max(0.0001,rms*0.38))
                return CGFloat(min(1, voice*(0.58+0.42*bandPresence)))
            }
            DispatchQueue.main.async { self.onLevels?(values) }
        }
        installed = true
        engine.prepare(); try engine.start()
    }

    func stop() {
        if installed { engine.inputNode.removeTap(onBus: 0); installed = false }
        engine.stop()
    }
}
