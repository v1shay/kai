import AppKit
import ImageIO

@MainActor final class SpriteAssetCache {
    private final class Entry:NSObject {
        let image:CGImage
        init(_ image:CGImage){self.image=image}
    }
    private let cache=NSCache<NSURL,Entry>()
    private let maxPixelSize:Int?
    private let queue=DispatchQueue(label:"kai.sprite-decode",qos:.userInitiated)
    private var pending=[URL:[(CGImage?)->Void]]()
    init(maxPixelSize:Int?=nil){self.maxPixelSize=maxPixelSize;cache.countLimit=maxPixelSize == nil ? 6:64;cache.totalCostLimit=maxPixelSize == nil ? 32*1024*1024:24*1024*1024}
    func image(at url:URL)->CGImage? {cache.object(forKey:url as NSURL)?.image}
    func load(_ url:URL,completion:@escaping(CGImage?)->Void) {
        if let image=image(at:url){completion(image);return}
        if pending[url] != nil {pending[url]?.append(completion);return}
        pending[url]=[completion]
        queue.async { [self] in
            let source=CGImageSourceCreateWithURL(url as CFURL,nil)
            let image=source.flatMap { source -> CGImage? in
                if let maxPixelSize {
                    return CGImageSourceCreateThumbnailAtIndex(source,0,[kCGImageSourceCreateThumbnailFromImageAlways:true,kCGImageSourceCreateThumbnailWithTransform:true,kCGImageSourceThumbnailMaxPixelSize:maxPixelSize,kCGImageSourceShouldCacheImmediately:true] as CFDictionary)
                }
                return CGImageSourceCreateImageAtIndex(source,0,[kCGImageSourceShouldCacheImmediately:true] as CFDictionary)
            }
            DispatchQueue.main.async { [self] in
                if let image {self.cache.setObject(Entry(image),forKey:url as NSURL,cost:image.bytesPerRow*image.height)}
                let callbacks=self.pending.removeValue(forKey:url) ?? []
                callbacks.forEach{$0(image)}
            }
        }
    }
}
