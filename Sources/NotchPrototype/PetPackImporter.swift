import AppKit
import ImageIO
import CryptoKit
import UniformTypeIdentifiers

/// Reads named archive members without extracting paths or running pack contents.
/// Conversion uses ImageIO/CoreGraphics; no Python, image editor or network dependency.
enum PetPackImporter {
    static let motions=["idle","running-right","running-left","waving","jumping","failed","waiting","running","review"]
    struct Result {let id:String;let folder:URL;let reused:Bool}
    enum Failure:LocalizedError {
        case invalid(String)
        var errorDescription:String?{if case .invalid(let text)=self{return text};return nil}
    }
    static func storage(home:URL=FileManager.default.homeDirectoryForCurrentUser)->URL{home.appendingPathComponent("Library/Application Support/Kai Unified/Pets")}
    static func fingerprint(_ url:URL)->String {
        let attrs=(try? FileManager.default.attributesOfItem(atPath:url.path)) ?? [:]
        return "\(url.standardizedFileURL.path)|\(attrs[.size] ?? 0)|\((attrs[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0)"
    }
    static func slug(_ text:String)->String {
        let value=text.lowercased().unicodeScalars.map{CharacterSet.alphanumerics.contains($0) && $0.isASCII ? String($0):"-"}.joined().split(separator:"-").joined(separator:"-")
        return String(value.prefix(100))
    }
    static func boundedRead(_ url:URL,limit:Int)throws->Data {
        guard let size=try url.resourceValues(forKeys:[.fileSizeKey]).fileSize,size>0,size<=limit else{throw Failure.invalid("Pet file exceeds its size budget")}
        return try Data(contentsOf:url)
    }
    private static func unzip(_ args:[String],limit:Int)throws->Data {
        let task=Process(),pipe=Pipe();task.executableURL=URL(fileURLWithPath:"/usr/bin/unzip");task.arguments=args;task.standardOutput=pipe;task.standardError=FileHandle.nullDevice;task.standardInput=FileHandle.nullDevice
        try task.run();var output=Data()
        while true {let bytes=pipe.fileHandleForReading.readData(ofLength:65536);if bytes.isEmpty{break};if output.count+bytes.count>limit{task.terminate();try? pipe.fileHandleForReading.close();task.waitUntilExit();throw Failure.invalid("Pet archive member exceeds its size budget")};output.append(bytes)}
        task.waitUntilExit();guard task.terminationStatus==0 else{throw Failure.invalid("Pet archive could not be read")};return output
    }
    private static func names(_ url:URL)throws->[String] {
        guard let size=try url.resourceValues(forKeys:[.fileSizeKey]).fileSize,size>0,size<=64*1024*1024 else{throw Failure.invalid("Pet archive exceeds its size budget")}
        let raw=try unzip(["-Z1",url.path],limit:128*1024)
        guard let text=String(data:raw,encoding:.utf8)else{throw Failure.invalid("Invalid archive paths")}
        let files=text.split(whereSeparator:\.isNewline).map(String.init).filter{!$0.hasSuffix("/") && !$0.hasPrefix("__MACOSX/")}
        guard files.count<=128,files.allSatisfy({!$0.hasPrefix("/") && !$0.contains("\\") && !$0.split(separator:"/").contains("..") && !$0.contains(where:{$0.isNewline || $0=="\0" || $0=="[" || $0=="*" || $0=="?"})}) else{throw Failure.invalid("Unsupported archive structure")}
        return files
    }
    static func imageInfo(_ data:Data)throws->(CGImageSource,Int,Int) {
        guard let source=CGImageSourceCreateWithData(data as CFData,[kCGImageSourceShouldCache:false] as CFDictionary),let props=CGImageSourceCopyPropertiesAtIndex(source,0,nil) as? [CFString:Any],let w=props[kCGImagePropertyPixelWidth] as? Int,let h=props[kCGImagePropertyPixelHeight] as? Int,w>0,h>0,w<=8192,h<=8192,w*h<=16*1024*1024 else{throw Failure.invalid("Unsupported or oversized pet image")}
        return (source,w,h)
    }
    static func validateSheet(_ data:Data)throws->Int {
        let (_,w,h)=try imageInfo(data);let cell=w/8,height=cell*208/192
        guard w%8==0,cell*208%192==0,height>0,h%height==0,[9,11].contains(h/height) else{throw Failure.invalid("Pet sheet must contain eight columns and nine or eleven 192:208 animation rows")}
        return h/height
    }
    private static func gifSheet(_ files:[String],read:(String)throws->Data)throws->(Data,[String:Motion]) {
        guard let context=CGContext(data:nil,width:1536,height:1872,bitsPerComponent:8,bytesPerRow:1536*4,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)else{throw Failure.invalid("Could not allocate pet sheet")}
        context.clear(CGRect(x:0,y:0,width:1536,height:1872));context.interpolationQuality = .high
        var overrides=[String:Motion]()
        for (row,name) in motions.enumerated(){
            guard let file=files.first(where:{URL(fileURLWithPath:$0).deletingPathExtension().lastPathComponent.hasSuffix("-"+name)})else{throw Failure.invalid("GIF pack is missing the \(name) animation")}
            let data=try read(file);let(source,_,_)=try imageInfo(data);let count=CGImageSourceGetCount(source)
            guard (1...8).contains(count)else{throw Failure.invalid("GIF animation has more than eight frames; it was not truncated")}
            var durations=[Int]()
            for column in 0..<count {
                guard let image=CGImageSourceCreateImageAtIndex(source,column,nil)else{throw Failure.invalid("Could not decode GIF frame")}
                let properties=CGImageSourceCopyPropertiesAtIndex(source,column,nil) as? [CFString:Any],gif=properties?[kCGImagePropertyGIFDictionary] as? [CFString:Any]
                let seconds=(gif?[kCGImagePropertyGIFUnclampedDelayTime] as? Double) ?? (gif?[kCGImagePropertyGIFDelayTime] as? Double) ?? 0.1
                durations.append(max(16,min(60_000,Int((seconds*1000).rounded()))))
                let ratio=min(192/CGFloat(image.width),208/CGFloat(image.height)),w=CGFloat(image.width)*ratio,h=CGFloat(image.height)*ratio
                context.draw(image,in:CGRect(x:CGFloat(column*192)+(192-w)/2,y:CGFloat((8-row)*208)+(208-h)/2,width:w,height:h))
            }
            overrides[name]=Motion(row:row,columns:Array(0..<count),durationsMs:durations,loop:!["waving","jumping","failed"].contains(name))
        }
        guard let image=context.makeImage()else{throw Failure.invalid("Could not make pet sheet")}
        let data=NSMutableData();guard let target=CGImageDestinationCreateWithData(data,UTType.png.identifier as CFString,1,nil)else{throw Failure.invalid("Could not encode pet sheet")};CGImageDestinationAddImage(target,image,nil);guard CGImageDestinationFinalize(target)else{throw Failure.invalid("Could not save pet sheet")};return(data as Data,overrides)
    }
    private static func folderFiles(_ folder:URL)throws->[String]{
        let base=folder.resolvingSymlinksInPath().standardizedFileURL
        guard let enumerator=FileManager.default.enumerator(at:base,includingPropertiesForKeys:[.isDirectoryKey,.isSymbolicLinkKey],options:.skipsHiddenFiles)else{throw Failure.invalid("Pet folder could not be read")}
        var files=[String]()
        for case let file as URL in enumerator {
            let values=try file.resourceValues(forKeys:[.isDirectoryKey,.isSymbolicLinkKey])
            if values.isSymbolicLink==true{enumerator.skipDescendants();continue}
            let canonical=file.resolvingSymlinksInPath().standardizedFileURL.path
            guard canonical.hasPrefix(base.path+"/")else{throw Failure.invalid("Pet file leaves its folder")}
            let name=String(canonical.dropFirst(base.path.count+1))
            if name.split(separator:"/").count>3{enumerator.skipDescendants();continue}
            if values.isDirectory != true{files.append(name)}
            if files.count>128{throw Failure.invalid("Pet folder contains too many files")}
        }
        return files
    }
    @discardableResult static func install(_ url:URL,to root:URL,alternateFor:String?=nil)throws->Result {
        let fm=FileManager.default
        var object=[String:Any](),imageData:Data,extensionName="png",overrides:[String:Motion]?=nil
        let isFolder=(try? url.resourceValues(forKeys:[.isDirectoryKey]).isDirectory)==true
        if url.pathExtension.lowercased()=="zip" || isFolder {
            let files = try isFolder ? folderFiles(url):names(url)
            let read:(String)throws->Data={name in
                if isFolder {let file=url.appendingPathComponent(name).resolvingSymlinksInPath();guard file.path.hasPrefix(url.resolvingSymlinksInPath().path+"/")else{throw Failure.invalid("Pet file leaves its folder")};return try boundedRead(file,limit:32*1024*1024)}
                return try unzip(["-p",url.path,name],limit:32*1024*1024)
            }
            if let manifest=files.first(where:{$0.hasSuffix("pet.json")}) {
                let data=try read(manifest);guard data.count<=64*1024,let decoded=try JSONSerialization.jsonObject(with:data) as? [String:Any]else{throw Failure.invalid("Invalid pet manifest")};object=decoded
                let directory=(manifest as NSString).deletingLastPathComponent
                let declared=object["spritesheetPath"] as? String ?? ""
                guard !declared.hasPrefix("/"),!declared.contains("\\"),!declared.split(separator:"/").contains("..")else{throw Failure.invalid("Pet image leaves its archive")}
                let expected=directory.isEmpty ? declared:directory+"/"+declared
                let alternatives=files.filter{["png","webp"].contains(URL(fileURLWithPath:$0).pathExtension.lowercased()) && URL(fileURLWithPath:$0).deletingPathExtension().lastPathComponent=="spritesheet"}
                guard let path=files.contains(expected) ? expected:(alternatives.count==1 ? alternatives[0]:nil)else{throw Failure.invalid("The pet manifest does not identify a sheet")}
                imageData=try read(path);extensionName=URL(fileURLWithPath:path).pathExtension.lowercased()
                if let value=object["animationOverrides"],let bytes=try? JSONSerialization.data(withJSONObject:value){overrides=try JSONDecoder().decode([String:Motion].self,from:bytes)}
            }else{
                let converted=try gifSheet(files,read:read);imageData=converted.0;overrides=converted.1
                let prefix=slug(url.deletingPathExtension().lastPathComponent).replacingOccurrences(of:"-gifs",with:"").replacingOccurrences(of:"-workload",with:"")
                object=["id":prefix,"displayName":prefix=="mr-bean" ? "Mr Bean":prefix.capitalized]
            }
        }else{imageData=try boundedRead(url,limit:32*1024*1024);extensionName=url.pathExtension.lowercased()}
        let rows=try validateSheet(imageData),digest=SHA256.hash(data:imageData).map{String(format:"%02x",$0)}.joined()
        try fm.createDirectory(at:root,withIntermediateDirectories:true)
        if let alternateFor {
            let folder=root.appendingPathComponent(slug(alternateFor));guard fm.fileExists(atPath:folder.appendingPathComponent("pet.json").path)else{throw Failure.invalid("Alternate sheet has no installed character")}
            try imageData.write(to:folder.appendingPathComponent("original-spritesheet."+extensionName),options:.atomic)
            record(url,id:alternateFor,root:root);return Result(id:alternateFor,folder:folder,reused:true)
        }
        var id=slug(object["id"] as? String ?? "")
        if id.isEmpty{id="local-"+digest.prefix(12)}
        var folder=root.appendingPathComponent(id)
        let peers=(try? fm.contentsOfDirectory(at:root,includingPropertiesForKeys:nil,options:.skipsHiddenFiles)) ?? []
        for peer in peers {
            if let data=try? Data(contentsOf:peer.appendingPathComponent("pet.json")),let meta=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any],meta["kaiImageSHA256"] as? String==digest,let prior=meta["id"] as? String {record(url,id:prior,root:root);return Result(id:prior,folder:peer,reused:true)}
        }
        if fm.fileExists(atPath:folder.path){id += "-"+digest.prefix(8);folder=root.appendingPathComponent(id)}
        let staging=root.appendingPathComponent(".import-"+UUID().uuidString);try fm.createDirectory(at:staging,withIntermediateDirectories:true);defer{try? fm.removeItem(at:staging)}
        let imageName="spritesheet."+extensionName;try imageData.write(to:staging.appendingPathComponent(imageName),options:.atomic)
        object["id"]=id;object["displayName"]=String((object["displayName"] as? String ?? "Local Pet \(digest.prefix(6))").prefix(120));object["spritesheetPath"]=imageName;object["kaiImageSHA256"]=digest
        object["category"]=PetCategory.category(id:id,name:object["displayName"] as! String,kind:object["kind"] as? String).rawValue
        if let overrides {for motion in overrides.values{guard motion.columns.count<=128,motion.durationsMs.allSatisfy({(16...60000).contains($0)}),SpriteTimeline(row:motion.row,rows:rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop) != nil else{throw Failure.invalid("Invalid animation override")}};object["animationOverrides"]=try JSONSerialization.jsonObject(with:JSONEncoder().encode(overrides))}
        try JSONSerialization.data(withJSONObject:object,options:[.prettyPrinted,.sortedKeys]).write(to:staging.appendingPathComponent("pet.json"),options:.atomic)
        try fm.moveItem(at:staging,to:folder);record(url,id:id,root:root);return Result(id:id,folder:folder,reused:false)
    }
    private static func ledger(_ root:URL)->[String:String]{guard let data=try? Data(contentsOf:root.appendingPathComponent(".import-index.json"))else{return [:]};return (try? JSONDecoder().decode([String:String].self,from:data)) ?? [:]}
    private static func record(_ url:URL,id:String,root:URL){var values=ledger(root);values[url.path]=fingerprint(url)+"|"+id;if let data=try? JSONEncoder().encode(values){try? data.write(to:root.appendingPathComponent(".import-index.json"),options:.atomic)}}
    static func importDownloads(_ downloads:URL,to root:URL){
        let fm=FileManager.default;try? fm.createDirectory(at:root,withIntermediateDirectories:true)
        let previous=ledger(root)
        let files=((try? fm.contentsOfDirectory(at:downloads,includingPropertiesForKeys:[.contentModificationDateKey],options:.skipsHiddenFiles)) ?? []).filter{(fm.fileExists(atPath:$0.appendingPathComponent("pet.json").path) || $0.pathExtension.lowercased()=="zip" || (["webp","png"].contains($0.pathExtension.lowercased()) && $0.lastPathComponent.lowercased().contains("spritesheet"))) && !$0.lastPathComponent.hasPrefix(".")}.sorted{let a=(try? $0.resourceValues(forKeys:[.contentModificationDateKey]).contentModificationDate) ?? .distantPast,b=(try? $1.resourceValues(forKeys:[.contentModificationDateKey]).contentModificationDate) ?? .distantPast;return a==b ? $0.path<$1.path:a>b}.prefix(128)
        for url in files {if previous[url.path]?.hasPrefix(fingerprint(url)+"|")==true{continue};do{_ = try autoreleasepool{try install(url,to:root)}}catch{record(url,id:"not-a-pet",root:root)}}
    }
}
