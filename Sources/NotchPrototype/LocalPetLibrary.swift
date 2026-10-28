import AppKit
import ImageIO

enum LocalPetLibrary {
    static func roots(defaults:UserDefaults = .standard,home:URL=FileManager.default.homeDirectoryForCurrentUser)->[URL]{
        var roots=[home.appendingPathComponent(".codex/pets"),home.appendingPathComponent("Library/Application Support/Kai Unified/Pets")]
        if let folder=defaults.string(forKey:"localPetFolder"),!folder.isEmpty{roots.append(URL(fileURLWithPath:folder))}
        return roots
    }
    static func discover(in roots:[URL])->[Pet]{
        var pets=[String:Pet]()
        for root in roots {
            let children=(try? FileManager.default.contentsOfDirectory(at:root,includingPropertiesForKeys:[.isDirectoryKey],options:.skipsHiddenFiles)) ?? []
            for folder in ([root]+children.sorted{$0.path<$1.path}).prefix(257){
                let manifest=folder.appendingPathComponent("pet.json")
                guard let size=try? manifest.resourceValues(forKeys:[.fileSizeKey]).fileSize,size<=64*1024,
                      let data=try? Data(contentsOf:manifest),let object=try? JSONSerialization.jsonObject(with:data) as? [String:Any],
                      let id=object["id"] as? String,!id.isEmpty,id.utf8.count<=128,!id.contains(where:{$0.isNewline || $0 == "/" || $0 == "\\" || $0 == "\0"}),
                      let path=object["spritesheetPath"] as? String,!path.isEmpty,!path.hasPrefix("/") else{continue}
                let parent=folder.resolvingSymlinksInPath().standardizedFileURL,image=parent.appendingPathComponent(path).resolvingSymlinksInPath().standardizedFileURL
                guard image.path.hasPrefix(parent.path+"/"),let bytes=try? image.resourceValues(forKeys:[.fileSizeKey]).fileSize,bytes<=32*1024*1024,
                      let source=CGImageSourceCreateWithURL(image as CFURL,[kCGImageSourceShouldCache:false] as CFDictionary),
                      let props=CGImageSourceCopyPropertiesAtIndex(source,0,nil) as? [CFString:Any],let width=props[kCGImagePropertyPixelWidth] as? Int,let height=props[kCGImagePropertyPixelHeight] as? Int,
                      width>0,height>0,width<=8192,height<=8192,width*height<=16*1024*1024,width%8 == 0 else{continue}
                let frameWidth=width/8,frameHeight=frameWidth*208/192
                guard frameHeight>0,height%frameHeight == 0,[9,11].contains(height/frameHeight) else{continue}
                pets[id]=Pet(id:id,displayName:(object["displayName"] as? String).map{String($0.prefix(120))} ?? id,spritesheet:path,grid:Grid(rows:height/frameHeight),animationOverrides:nil,sourceURL:image,kind:"local")
            }
        }
        return pets.values.sorted{$0.id<$1.id}
    }
    static func merge(_ local:[Pet],into bundled:Catalog)->Catalog{
        var pets=bundled.pets.filter{$0.kind != "installed_custom"}
        for pet in local {
            if let index=pets.firstIndex(where:{$0.id == pet.id}){pets[index]=pet}else{pets.append(pet)}
        }
        return Catalog(pets:pets,stateAnimations:bundled.stateAnimations)
    }
    static func migratedID(_ id:String,in bundled:Catalog,local:[Pet])->String{
        guard let old=bundled.pets.first(where:{$0.id == id && $0.kind == "installed_custom"}) else{return id}
        let folder=URL(fileURLWithPath:old.spritesheet).deletingLastPathComponent().lastPathComponent
        return local.first{$0.sourceURL?.deletingLastPathComponent().lastPathComponent == folder}?.id ?? id
    }
    static func profiles(_ pets:[Pet])->[String:PetGradientProfile]{
        var result=[String:PetGradientProfile]()
        for pet in pets {
            guard let url=pet.sourceURL,let source=CGImageSourceCreateWithURL(url as CFURL,nil),let image=CGImageSourceCreateThumbnailAtIndex(source,0,[kCGImageSourceCreateThumbnailFromImageAlways:true,kCGImageSourceThumbnailMaxPixelSize:64,kCGImageSourceShouldCacheImmediately:true] as CFDictionary) else{continue}
            let bitmap=NSBitmapImageRep(cgImage:image);var buckets=[Int:Double]()
            for y in 0..<bitmap.pixelsHigh{for x in 0..<bitmap.pixelsWide{
                guard let c=bitmap.colorAt(x:x,y:y)?.usingColorSpace(.deviceRGB),c.alphaComponent>0.25 else{continue}
                let maximum=max(c.redComponent,max(c.greenComponent,c.blueComponent)),minimum=min(c.redComponent,min(c.greenComponent,c.blueComponent))
                guard maximum>0.12 else{continue}
                let key=(Int(c.redComponent*15)<<8)|(Int(c.greenComponent*15)<<4)|Int(c.blueComponent*15)
                buckets[key,default:0]+=Double(c.alphaComponent*(0.1+maximum-minimum))
            }}
            var colors=buckets.sorted{$0.value == $1.value ? $0.key<$1.key:$0.value>$1.value}.prefix(3).map{entry in let k=entry.key;return String(format:"#%02x%02x%02x",((k>>8)&15)*17,((k>>4)&15)*17,(k&15)*17)}
            guard !colors.isEmpty else{continue};while colors.count<3{colors.append(colors.last!)}
            let recipe=GradientRecipe(angleDegrees:25,cycleDurationMs:2200,stops:colors.enumerated().map{GradientStop(location:Double($0.offset)/2,color:$0.element)})
            result[pet.id]=PetGradientProfile(palette:PetPalette(shadow:colors[0],primary:colors[0],secondary:colors[1],accent:colors[2],highlight:"#ffffff",foreground:"#ffffff"),gradients:PetGradients(ambient:recipe,thinking:recipe,working:recipe,success:recipe,warning:recipe,error:recipe))
        }
        return result
    }
}
