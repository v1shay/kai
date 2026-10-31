import AppKit
import ImageIO
import CryptoKit

@main struct PetLibraryAudit {
    static func check(_ value:@autoclosure()->Bool,_ message:String){precondition(value(),message)}
    @MainActor static func main()throws {
        _=NSApplication.shared
        let args=CommandLine.arguments
        if let index=args.firstIndex(of:"--import-downloads"),args.count>index+2 {
            let downloads=URL(fileURLWithPath:args[index+1]),root=URL(fileURLWithPath:args[index+2])
            var report=[[String:Any]]()
            let formatter=DateFormatter();formatter.timeZone=TimeZone(identifier:"America/Los_Angeles");formatter.dateFormat="yyyy-MM-dd"
            let day=args.firstIndex(of:"--date").flatMap{args.count>$0+1 ? args[$0+1]:nil} ?? formatter.string(from:Date())
            for file in try FileManager.default.contentsOfDirectory(at:downloads,includingPropertiesForKeys:[.contentModificationDateKey]).sorted(by:{$0.path<$1.path}) where file.pathExtension=="zip" {
                let date=try file.resourceValues(forKeys:[.contentModificationDateKey]).contentModificationDate ?? .distantPast
                guard formatter.string(from:date)==day else{continue}
                let result=try autoreleasepool{try PetPackImporter.install(file,to:root)}
                report.append(["source":file.lastPathComponent,"id":result.id,"reused":result.reused])
            }
            for (name,id) in [("spritesheet.webp","clippy"),("spritesheet (1).webp","mr-bean")] {
                let file=downloads.appendingPathComponent(name)
                if FileManager.default.fileExists(atPath:file.path){let result=try PetPackImporter.install(file,to:root,alternateFor:id);report.append(["source":name,"id":result.id,"alternate":true])}
            }
            let pets=LocalPetLibrary.discover(in:[root]),profiles=LocalPetLibrary.profiles(pets)
            check(pets.count==14,"Expected fourteen distinct characters after merging duplicate archives and alternate formats")
            check(profiles.count==pets.count,"Every imported pet needs a palette")
            try JSONSerialization.data(withJSONObject:["sources":report,"uniquePets":pets.count,"profiles":profiles.count,"pets":pets.map{["id":$0.id,"name":$0.displayName,"category":PetCategory.of($0).rawValue,"rows":$0.grid.rows]}],options:[.prettyPrinted,.sortedKeys]).write(to:root.appendingPathComponent(".validation-report.json"),options:.atomic)
            print("Imported \(pets.count) unique pets from \(report.count) downloads with \(profiles.count) native gradient profiles")
            return
        }
        let scratch=FileManager.default.temporaryDirectory.appendingPathComponent("kai-pet-audit-"+UUID().uuidString)
        defer{try? FileManager.default.removeItem(at:scratch)}
        let source=scratch.appendingPathComponent("source"),installed=scratch.appendingPathComponent("installed")
        try FileManager.default.createDirectory(at:source,withIntermediateDirectories:true)
        let context=CGContext(data:nil,width:1536,height:1872,bitsPerComponent:8,bytesPerRow:6144,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.setFillColor(NSColor.systemBlue.cgColor);context.fill(CGRect(x:30,y:10,width:100,height:150))
        context.setFillColor(NSColor.systemPink.cgColor);context.fill(CGRect(x:70,y:1600,width:120,height:160))
        let bytes=NSMutableData(),target=CGImageDestinationCreateWithData(bytes,"public.png" as CFString,1,nil)!
        CGImageDestinationAddImage(target,context.makeImage()!,nil);check(CGImageDestinationFinalize(target),"Fixture image failed")
        try (bytes as Data).write(to:source.appendingPathComponent("spritesheet.png"))
        let manifest:[String:Any]=["id":"test-native","displayName":"Test native","spritesheetPath":"spritesheet.webp","kind":"person"]
        try JSONSerialization.data(withJSONObject:manifest).write(to:source.appendingPathComponent("pet.json"))
        let result=try PetPackImporter.install(source,to:installed)
        check(!result.reused,"First import was not installed")
        let duplicate=try PetPackImporter.install(source,to:installed);check(duplicate.reused,"Repeated import created duplicates")
        let pets=LocalPetLibrary.discover(in:[installed]);check(pets.count==1,"Installed PNG with a broken WebP manifest was not repaired")
        let profiles=LocalPetLibrary.profiles(pets);check(profiles.count==1,"Missing gradient")
        check(LocalPetLibrary.preferred([result.id,"../../escape"],root:installed).count==1,"Remembered pet was not safely restored")
        check(LocalPetLibrary.cachedProfiles(pets).count==1,"Remembered pet palette is not available without decoding its sheet")
        check((UInt(profiles[result.id]!.palette.primary.dropFirst(),radix:16)! >> 16)>40,"Palette lost its red channel")
        check(profiles[result.id]!.gradients.ambient.cycleDurationMs != profiles[result.id]!.gradients.working.cycleDurationMs,"Status gradients lost their individual timing")
        let first=LocalPetLibrary.fingerprint(pets);check(first==LocalPetLibrary.fingerprint(LocalPetLibrary.discover(in:[installed])),"Unchanged discovery fingerprint is unstable")
        let cache=pets[0].sourceURL!.deletingLastPathComponent().appendingPathComponent(".kai-gradient.json")
        var cacheData=try Data(contentsOf:cache);cacheData.append(Data("\n".utf8));try cacheData.write(to:cache,options:.atomic)
        check(first != LocalPetLibrary.fingerprint(pets),"Changed local gradients did not invalidate library metadata")
        // Reject a path escape without reading any outside asset.
        var bad=manifest;bad["id"]="escape";bad["spritesheetPath"]="../outside.png"
        let escape=installed.appendingPathComponent("escape");try FileManager.default.createDirectory(at:escape,withIntermediateDirectories:true)
        try JSONSerialization.data(withJSONObject:bad).write(to:escape.appendingPathComponent("pet.json"))
        check(LocalPetLibrary.discover(in:[installed]).count==1,"Escaping manifest was accepted")
        let gallery=PetGallery();let motions=Dictionary(uniqueKeysWithValues:PetPackImporter.motions.enumerated().map{($0.element,PetGalleryMotion(row:$0.offset,columns:[0],durationsMs:[100],loop:true))})
        let specs=(0..<40).map{n in PetGallerySpec(id:"pet-\(n)",spritesheet:"spritesheet.png",rows:9,motions:motions,colors:[NSColor.systemBlue.cgColor],sourceURL:pets[0].sourceURL,category:n%2==0 ? "Characters":"Sports",displayName:"Pet \(n)")}
        gallery.configure(specs,at:installed);gallery.layout(in:CGRect(x:0,y:0,width:470,height:320),selected:"pet-0");gallery.show(selected:"pet-0",motion:"idle")
        func shown()->Set<String>{Set((gallery.layer.sublayers ?? []).filter{!$0.isHidden && $0.name?.hasPrefix("gallery-pet:")==true}.compactMap(\.name))}
        let initial=shown();check(initial.count==12,"Gallery is not bounded to a twelve-pet page")
        check(gallery.handleNavigation(at:CGPoint(x:450,y:12),selected:"pet-0"),"Next page did not handle click")
        check(shown().count==12 && shown() != initial,"Page did not change")
        check(gallery.handleNavigation(at:CGPoint(x:117,y:306),selected:"pet-0"),"Category did not handle click")
        check(gallery.pet(at:CGPoint(x:117,y:306))==nil,"Category click also selected a pet")
        gallery.hide()
        let monitor=LocalPetMonitor();var events=0;monitor.changed={events+=1};monitor.watch([installed])
        try Data("test".utf8).write(to:installed.appendingPathComponent("changed"))
        let until=Date().addingTimeInterval(1.8);while Date()<until{RunLoop.main.run(until:Date().addingTimeInterval(0.01))}
        check(events==1,"Directory change did not coalesce into one discovery event");monitor.stop()
        print("Native pet library: repaired manifests, duplicate suppression, path containment, cached per-state palettes, twelve-pet pages, categories, isolated hit targets and event-driven discovery passed")
    }
}
