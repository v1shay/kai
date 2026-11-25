import AppKit

/// Foreground text colors only. No background, layer opacity, border, shape,
/// image, font, frame, appearance or glass property is ever written here.
@MainActor final class AdaptiveTextInk {
    weak var host:NSView?
    private var target:UInt32?,epoch=0,started=Date(),duration=0.0,applying=false
    private var task:DispatchWorkItem?
    private final class Source: NSObject {
        var original: NSColor?
        var applied: NSColor?
        var from: NSColor?
        var destination: NSColor?
        var epoch = -1
    }
    private struct ForegroundRun {
        let range: NSRange
        let original: NSColor
        let from: NSColor
        let destination: NSColor
        var applied: NSColor
        var needsMetadata: Bool
    }
    private final class StorageState: NSObject {
        let views = NSHashTable<NSTextView>(options: [.weakMemory, .objectPointerPersonality])
        var epoch = -1
        var dirty = true
        var runs = [ForegroundRun]()
        var indexedLength = 0
    }
    private let storages = NSMapTable<NSTextStorage, StorageState>(keyOptions: [.weakMemory, .objectPointerPersonality], valueOptions: .strongMemory)
    private var visitedStorages = Set<ObjectIdentifier>()
    private var visitedLayers = Set<ObjectIdentifier>()
    private var storageObserver: NSObjectProtocol?
    private var colorObserver: NSObjectProtocol?
    private var lastAppearanceName: NSAppearance.Name?
    private weak var writingStorage: NSTextStorage?
    private var refreshQueued = false
    #if DEBUG
    private(set) var diagnosticStorageBuildCount = 0
    private(set) var diagnosticStorageWriteCount = 0
    private(set) var diagnosticRefreshCount = 0
    private(set) var diagnosticStorageNotificationCount = 0
    private(set) var diagnosticHostStorageNotificationCount = 0
    #endif
    private let fields=NSMapTable<NSView,Source>(keyOptions:.weakMemory,valueOptions:.strongMemory)
    private let carets=NSMapTable<NSTextView,Source>(keyOptions:.weakMemory,valueOptions:.strongMemory)
    private static let original=NSAttributedString.Key("kaiTextOriginal"),applied=NSAttributedString.Key("kaiTextApplied"),from=NSAttributedString.Key("kaiTextFrom"),epochKey=NSAttributedString.Key("kaiTextEpoch")
    init(host: NSView) {
        self.host = host
        storageObserver = NotificationCenter.default.addObserver(
            forName: NSTextStorage.didProcessEditingNotification, object: nil, queue: nil
        ) { [weak self] note in
            guard let storage = note.object as? NSTextStorage else { return }
            if Thread.isMainThread {
                MainActor.assumeIsolated { self?.storageDidChange(storage) }
            } else {
                DispatchQueue.main.async { self?.storageDidChange(storage) }
            }
        }
        colorObserver = NotificationCenter.default.addObserver(
            forName: NSColor.systemColorsDidChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.invalidateEndpointColors()
                if self.applying { self.queueRefresh() } else { self.refresh() }
            }
        }
    }

    deinit {
        if let colorObserver { NotificationCenter.default.removeObserver(colorObserver) }
        if let storageObserver { NotificationCenter.default.removeObserver(storageObserver) }
    }

    private func invalidateEndpointColors() {
        for state in storages.objectEnumerator()?.allObjects as? [StorageState] ?? [] {
            state.dirty = true
        }
        for source in fields.objectEnumerator()?.allObjects as? [Source] ?? [] {
            source.destination = nil
        }
        for source in carets.objectEnumerator()?.allObjects as? [Source] ?? [] {
            source.destination = nil
        }
    }

    private func storageDidChange(_ storage: NSTextStorage) {
        #if DEBUG
        diagnosticStorageNotificationCount += 1
        #endif
        guard target != nil, let host, writingStorage !== storage else { return }
        let state = storages.object(forKey: storage)
        state?.dirty = true
        let indexedOwner = state?.views.allObjects.contains {
            $0 === host || $0.isDescendant(of: host)
        } ?? false
        let layoutOwner = storage.layoutManagers.contains { manager in
            manager.textContainers.contains { container in
                guard let view = container.textView else { return false }
                return view === host || view.isDescendant(of: host)
            }
        }
        // TextKit 2 storage can have zero legacy layoutManagers. Discover a newly
        // attached owner with a view-only walk, without forcing legacy TextKit.
        guard indexedOwner || layoutOwner || contains(storage, in: host) else { return }
        #if DEBUG
        diagnosticHostStorageNotificationCount += 1
        #endif
        if applying { queueRefresh() } else { refresh() }
    }
    private func contains(_ storage: NSTextStorage, in view: NSView) -> Bool {
        if let text = view as? NSTextView, text.textStorage === storage { return true }
        return view.subviews.contains { contains(storage, in: $0) }
    }

    func setNeedsRefresh() {
        guard target != nil else { return }
        queueRefresh()
    }

    private func queueRefresh() {
        guard !refreshQueued else { return }
        refreshQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.refreshQueued = false
            self.refresh()
        }
    }

    func apply(color:UInt32,animated:Bool){
        if target != color{target=color;epoch+=1;started=Date();duration=animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.24:0;task?.cancel();task=nil}
        refresh();schedule()
    }
    private func schedule(){
        guard target != nil,duration>0,Date().timeIntervalSince(started)<duration,task==nil else{return}
        let generation=epoch;let work=DispatchWorkItem{[weak self] in guard let self,self.epoch==generation,self.target != nil else{return};self.task=nil;self.refresh();self.schedule()};task=work;DispatchQueue.main.asyncAfter(deadline:.now()+1.0/60,execute:work)
    }
    private func adjusted(_ source:NSColor)->NSColor{
        guard let color=source.usingColorSpace(.sRGB),let target else{return source}
        let original=[color.redComponent,color.greenComponent,color.blueComponent]
        let ink=[CGFloat((target>>16)&255)/255,CGFloat((target>>8)&255)/255,CGFloat(target&255)/255]
        let neutral=(original.max() ?? 0)-(original.min() ?? 0)<0.10
        let rgb=(0..<3).map{neutral ? ink[$0]:original[$0]*0.4+ink[$0]*0.6}
        return NSColor(srgbRed:rgb[0],green:rgb[1],blue:rgb[2],alpha:color.alphaComponent)
    }
    private func mix(_ from:NSColor,_ to:NSColor,_ t:Double)->NSColor{
        guard let a=from.usingColorSpace(.sRGB),let b=to.usingColorSpace(.sRGB) else{return to}
        func lerp(_ a:CGFloat,_ b:CGFloat)->CGFloat{a+(b-a)*t}
        return NSColor(srgbRed:lerp(a.redComponent,b.redComponent),green:lerp(a.greenComponent,b.greenComponent),blue:lerp(a.blueComponent,b.blueComponent),alpha:lerp(a.alphaComponent,b.alphaComponent))
    }
    func refresh() { refresh(progress: nil) }

    #if DEBUG
    func diagnosticApply(color: UInt32, progress: Double) {
        task?.cancel(); task = nil
        if target != color { target = color; epoch += 1 }
        refresh(progress: progress)
    }
    #endif

    private func refresh(progress: Double?){
        guard !applying,target != nil,let host else{return};applying=true;defer{applying=false}
        #if DEBUG
        diagnosticRefreshCount += 1
        #endif
        let appearance = host.effectiveAppearance
        if lastAppearanceName != appearance.name {
            invalidateEndpointColors()
            lastAppearanceName = appearance.name
        }
        let x = progress ?? (duration > 0 ? min(1, max(0, Date().timeIntervalSince(started) / duration)) : 1)
        let t = x * x * (3 - 2 * x)
        visitedStorages.removeAll(keepingCapacity: true)
        visitedLayers.removeAll(keepingCapacity: true)
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        walk(host,t:t,restore:false)
    }
    func restore(){
        guard target != nil,!applying,let host else{return};task?.cancel();task=nil;applying=true;defer{applying=false};epoch+=1
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        visitedStorages.removeAll(keepingCapacity: true)
        visitedLayers.removeAll(keepingCapacity: true)
        walk(host,t:1,restore:true);target=nil;fields.removeAllObjects();carets.removeAllObjects();storages.removeAllObjects()
    }
    private func next(_ current:NSColor,source:Source,t:Double,restore:Bool)->NSColor{
        if source.applied==nil || current != source.applied{source.original=current;source.from=current;source.destination=nil}
        guard let original=source.original else{return current}
        if restore{return original}
        if source.epoch != epoch{source.from=current;source.epoch=epoch;source.destination=nil}
        if source.destination == nil { source.destination = adjusted(original) }
        let value=mix(source.from ?? current,source.destination ?? original,t);source.applied=value;return value
    }
    private func walk(_ view:NSView,t:Double,restore:Bool){
        if let field = view as? NSTextField, let color = field.textColor {
            let source = fields.object(forKey: field) ?? Source()
            let result = next(color, source: source, t: t, restore: restore)
            if result != color { field.textColor = result }
            if !restore { fields.setObject(source, forKey: field) }
        }
        if let text=view as? NSTextView{
            let source = carets.object(forKey: text) ?? Source()
            let result = next(text.insertionPointColor, source: source, t: t, restore: restore)
            if result != text.insertionPointColor { text.insertionPointColor = result }
            if !restore { carets.setObject(source, forKey: text) }
            if let storage = text.textStorage {
                let state = storages.object(forKey: storage) ?? StorageState()
                state.views.add(text)
                storages.setObject(state, forKey: storage)
                if visitedStorages.insert(ObjectIdentifier(storage)).inserted {
                    apply(to: storage, progress: t, restore: restore)
                }
            }
        }
        if let layer = view.layer { walk(layer, t: t, restore: restore) }
        for child in view.subviews{walk(child,t:t,restore:restore)}
    }
    private func apply(to storage: NSTextStorage, progress: Double, restore: Bool) {
        let state = storages.object(forKey: storage) ?? StorageState()
        if state.dirty || state.epoch != epoch {
            state.runs.removeAll(keepingCapacity: true)
            storage.enumerateAttributes(in: NSRange(location: 0, length: storage.length)) { attributes, range, _ in
                guard let current = attributes[.foregroundColor] as? NSColor else { return }
                let previous = attributes[Self.applied] as? NSColor
                let original = (previous != nil && previous != current)
                    ? current : (attributes[Self.original] as? NSColor ?? current)
                let from = (attributes[Self.epochKey] as? Int) == epoch
                    ? (attributes[Self.from] as? NSColor ?? current) : current
                state.runs.append(ForegroundRun(
                    range: range, original: original, from: from,
                    destination: adjusted(original), applied: current,
                    needsMetadata: restore ? attributes[Self.original] != nil : attributes[Self.original] == nil
                ))
            }
            state.dirty = false
            state.indexedLength = storage.length
            state.epoch = epoch
            storages.setObject(state, forKey: storage)
            #if DEBUG
            diagnosticStorageBuildCount += 1
            #endif
        }
        var changes = [(Int, NSColor)]()
        for index in state.runs.indices {
            let run = state.runs[index]
            let result = restore ? run.original : mix(run.from, run.destination, progress)
            if result != run.applied || run.needsMetadata { changes.append((index, result)) }
        }
        guard !changes.isEmpty else { return }
        writingStorage = storage
        storage.beginEditing()
        for (index, result) in changes {
            let run = state.runs[index]
            storage.addAttribute(.foregroundColor, value: result, range: run.range)
            if restore {
                for key in [Self.original, Self.applied, Self.from, Self.epochKey] {
                    storage.removeAttribute(key, range: run.range)
                }
            } else {
                storage.addAttributes([
                    Self.original: run.original, Self.applied: result,
                    Self.from: run.from, Self.epochKey: epoch
                ], range: run.range)
            }
            state.runs[index].applied = result
            state.runs[index].needsMetadata = false
            #if DEBUG
            diagnosticStorageWriteCount += 1
            #endif
        }
        storage.endEditing()
        writingStorage = nil
        if storage.length != state.indexedLength {
            state.dirty = true
            queueRefresh()
            return
        }
        // Other notification observers can commit a partial foreground edit while
        // endEditing delivers our notification. Verify only the cached color runs.
        // The complete attributed document is indexed only after an invalidation.
        for run in state.runs {
            guard NSMaxRange(run.range) <= storage.length else {
                state.dirty = true
                break
            }
            var effective = NSRange()
            let current = storage.attribute(
                .foregroundColor, at: run.range.location,
                longestEffectiveRange: &effective, in: run.range
            ) as? NSColor
            if current != run.applied || effective != run.range {
                state.dirty = true
                break
            }
            if !restore && (
                !attribute(Self.original, matches: run.original, in: run.range, storage: storage) ||
                !attribute(Self.applied, matches: run.applied, in: run.range, storage: storage) ||
                !attribute(Self.from, matches: run.from, in: run.range, storage: storage) ||
                !attribute(Self.epochKey, matches: NSNumber(value: epoch), in: run.range, storage: storage)
            ) {
                state.dirty = true
                break
            }
        }
        if state.dirty { queueRefresh() }
    }

    private func attribute(_ key: NSAttributedString.Key, matches expected: NSObject,
                           in range: NSRange, storage: NSTextStorage) -> Bool {
        var effective = NSRange()
        let current = storage.attribute(key, at: range.location,
            longestEffectiveRange: &effective, in: range) as? NSObject
        return effective == range && current?.isEqual(expected) == true
    }

    private func walk(_ layer:CALayer,t:Double,restore:Bool){
        guard visitedLayers.insert(ObjectIdentifier(layer)).inserted else { return }
        if let text=layer as? CATextLayer,let current=text.foregroundColor,let color=NSColor(cgColor:current){
            let previous=AdaptiveColorMath.cgColor(layer.value(forKey:"kaiTextApplied")),stored=AdaptiveColorMath.cgColor(layer.value(forKey:"kaiTextOriginal"))
            let original=(previous != nil && previous != current) ? current:(stored ?? current)
            let from=(layer.value(forKey:"kaiTextEpoch") as? Int)==epoch ? (AdaptiveColorMath.cgColor(layer.value(forKey:"kaiTextFrom")) ?? current):current
            let destination: CGColor
            if (layer.value(forKey: "kaiTextEpoch") as? Int) == epoch,
               previous == current,
               let cached = AdaptiveColorMath.cgColor(layer.value(forKey: "kaiTextDestination")) {
                destination = cached
            } else {
                destination = adjusted(NSColor(cgColor: original) ?? color).cgColor
            }
            let result=restore ? original:mix(NSColor(cgColor:from) ?? color,NSColor(cgColor:destination) ?? color,t).cgColor
            if result != current { text.foregroundColor = result }
            if restore{for key in ["kaiTextOriginal","kaiTextApplied","kaiTextFrom","kaiTextEpoch","kaiTextDestination"]{layer.setValue(nil,forKey:key)}}else{layer.setValue(original,forKey:"kaiTextOriginal");layer.setValue(result,forKey:"kaiTextApplied");layer.setValue(from,forKey:"kaiTextFrom");layer.setValue(epoch,forKey:"kaiTextEpoch");layer.setValue(destination,forKey:"kaiTextDestination")}
        }
        // Masks are not part of sublayers and are intentionally never traversed.
        for child in layer.sublayers ?? []{walk(child,t:t,restore:restore)}
    }
}
