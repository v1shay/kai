import AppKit
import ImageIO
import CryptoKit

enum LocalPetLibrary {
    static func roots(defaults:UserDefaults = .standard,home:URL=FileManager.default.homeDirectoryForCurrentUser)->[URL]{
        var roots=[home.appendingPathComponent("Downloads"),home.appendingPathComponent(".codex/pets"),PetPackImporter.storage(home:home),home.appendingPathComponent("Library/Application Support/Code/Pets"),home.appendingPathComponent("Library/Application Support/Kai/Pets")]
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
                let overrides:[String:Motion]?
                if let value=object["animationOverrides"],let bytes=try? JSONSerialization.data(withJSONObject:value){
                    guard let decoded=try? JSONDecoder().decode([String:Motion].self,from:bytes),decoded.values.allSatisfy({$0.columns.count<=128 && $0.durationsMs.allSatisfy{(16...60000).contains($0)} && SpriteTimeline(row:$0.row,rows:height/frameHeight,columns:$0.columns,durationsMs:$0.durationsMs,loops:$0.loop) != nil})else{continue};overrides=decoded
                }else{overrides=nil}
                pets[id]=Pet(id:id,displayName:(object["displayName"] as? String).map{String($0.prefix(120))} ?? id,spritesheet:path,grid:Grid(rows:height/frameHeight),animationOverrides:overrides,sourceURL:image,kind:object["kind"] as? String,category:object["category"] as? String)
            }
        }
        return pets.values.sorted{$0.id<$1.id}
    }
    static func fingerprint(_ pets:[Pet])->String {
        let encoder=JSONEncoder();encoder.outputFormatting = .sortedKeys
        return pets.map{pet in
            var fields=[pet.id,pet.displayName,pet.category ?? "",pet.kind ?? ""]
            fields.append(pet.sourceURL.map{PetPackImporter.fingerprint($0)} ?? "")
            fields.append(pet.sourceURL.map{PetPackImporter.fingerprint($0.deletingLastPathComponent().appendingPathComponent(".kai-gradient.json"))} ?? "")
            fields.append((try? encoder.encode(pet.animationOverrides))?.base64EncodedString() ?? "")
            return fields.joined(separator:"|")
        }.joined(separator:"\n")
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
    private struct CachedProfile:Codable {let fingerprint:String;let profile:PetGradientProfile}
    static func preferred(_ ids:[String],root:URL=PetPackImporter.storage())->[Pet]{
        var seen=Set<String>()
        return ids.filter{!$0.isEmpty && PetPackImporter.slug($0)==$0 && seen.insert($0).inserted}.flatMap{id in discover(in:[root.appendingPathComponent(id)]).filter{$0.id==id}}
    }
    static func cachedProfiles(_ pets:[Pet])->[String:PetGradientProfile]{
        var result=[String:PetGradientProfile]()
        for pet in pets {guard let url=pet.sourceURL,let data=try? Data(contentsOf:url.deletingLastPathComponent().appendingPathComponent(".kai-gradient.json")),data.count<64*1024,let saved=try? JSONDecoder().decode(CachedProfile.self,from:data),saved.fingerprint==PetPackImporter.fingerprint(url)else{continue};result[pet.id]=saved.profile}
        return result
    }
    static func profiles(_ pets:[Pet],cache persist:Bool=true)->[String:PetGradientProfile]{
        var result=[String:PetGradientProfile]()
        for pet in pets {autoreleasepool{
            guard let url=pet.sourceURL else{return}
            let fingerprint=PetPackImporter.fingerprint(url),cache=url.deletingLastPathComponent().appendingPathComponent(".kai-gradient.json")
            if let data=try? Data(contentsOf:cache),let saved=try? JSONDecoder().decode(CachedProfile.self,from:data),saved.fingerprint==fingerprint{result[pet.id]=saved.profile;return}
            guard let source=CGImageSourceCreateWithURL(url as CFURL,[kCGImageSourceShouldCache:false] as CFDictionary),let image=CGImageSourceCreateThumbnailAtIndex(source,0,[kCGImageSourceCreateThumbnailFromImageAlways:true,kCGImageSourceThumbnailMaxPixelSize:768,kCGImageSourceShouldCacheImmediately:true] as CFDictionary)else{return}
            let cellWidth=image.width/8,cellHeight=image.height/pet.grid.rows
            guard let neutral=image.cropping(to:CGRect(x:0,y:0,width:cellWidth,height:cellHeight)),let context=CGContext(data:nil,width:32,height:32,bitsPerComponent:8,bytesPerRow:128,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)else{return}
            context.draw(neutral,in:CGRect(x:0,y:0,width:32,height:32));guard let bytes=context.data?.assumingMemoryBound(to:UInt8.self)else{return}
            var buckets=[Int:Double]()
            for n in 0..<1024 {let i=n*4,a=Double(bytes[i+3])/255;guard a>0.8 else{continue};let r=min(255,Double(bytes[i])/a),g=min(255,Double(bytes[i+1])/a),b=min(255,Double(bytes[i+2])/a);let maximum=max(r,max(g,b)),minimum=min(r,min(g,b));guard maximum>24 else{continue};let key=((Int(r)/17)<<8)|((Int(g)/17)<<4)|(Int(b)/17);buckets[key,default:0]+=a*(0.3+(maximum-minimum)/255)}
            let ranked=buckets.sorted{$0.value == $1.value ? $0.key<$1.key:$0.value>$1.value}
            guard !ranked.isEmpty else{return}
            func rgb(_ key:Int)->(CGFloat,CGFloat,CGFloat){(CGFloat((key>>8)&15)/15,CGFloat((key>>4)&15)/15,CGFloat(key&15)/15)}
            func hex(_ key:Int)->String{let(r,g,b)=rgb(key);return String(format:"#%02x%02x%02x",Int(r*255),Int(g*255),Int(b*255))}
            func luminance(_ key:Int)->CGFloat{let(r,g,b)=rgb(key);return r*0.2126+g*0.7152+b*0.0722}
            let primary=ranked[0].key
            let secondary=ranked.first{entry in let(a,b,c)=rgb(entry.key),(r,g,v)=rgb(primary);return abs(a-r)+abs(b-g)+abs(c-v)>0.45}?.key ?? ranked.last!.key
            let accent=ranked.prefix(12).max{a,b in let(r,g,v)=rgb(a.key),(x,y,z)=rgb(b.key);return max(r,max(g,v))-min(r,min(g,v)) < max(x,max(y,z))-min(x,min(y,z))}!.key
            let viable=ranked.prefix(16),shadow=viable.min{luminance($0.key)<luminance($1.key)}!.key,highlight=viable.max{luminance($0.key)<luminance($1.key)}!.key
            let p=hex(primary),s=hex(secondary),a=hex(accent),dark=hex(shadow),light=hex(highlight)
            func recipe(_ angle:Double,_ duration:Int,_ colors:[String])->GradientRecipe{GradientRecipe(angleDegrees:angle,cycleDurationMs:duration,stops:colors.enumerated().map{GradientStop(location:Double($0.offset)/Double(max(1,colors.count-1)),color:$0.element)})}
            let profile=PetGradientProfile(palette:PetPalette(shadow:dark,primary:p,secondary:s,accent:a,highlight:light,foreground:luminance(primary)>0.6 ? "#000000":"#ffffff"),gradients:PetGradients(ambient:recipe(90,3200,[dark,p,a]),thinking:recipe(115,1500,[p,a,light,p]),working:recipe(0,1100,[dark,p,s,a]),success:recipe(25,900,[dark,a,light]),warning:recipe(25,900,[dark,s,a,light]),error:recipe(25,900,[dark,s,a])))
            result[pet.id]=profile
            if persist,let data=try? JSONEncoder().encode(CachedProfile(fingerprint:fingerprint,profile:profile)){try? data.write(to:cache,options:.atomic)}
        }}
        return result
    }
}
