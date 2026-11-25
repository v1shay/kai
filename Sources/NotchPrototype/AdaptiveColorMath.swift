import AppKit

enum AdaptiveColorMath {
    static func cgColor(_ value:Any?)->CGColor?{guard let value else{return nil};let cf=value as CFTypeRef;guard CFGetTypeID(cf)==CGColor.typeID else{return nil};return unsafeBitCast(cf,to:CGColor.self)}
    static func cgImage(_ value:Any?)->CGImage?{guard let value else{return nil};let cf=value as CFTypeRef;guard CFGetTypeID(cf)==CGImage.typeID else{return nil};return unsafeBitCast(cf,to:CGImage.self)}
    /// Normalized screen points make Retina scale and negative display origins irrelevant.
    static func sampleBGRA(_ bytes:[UInt8],width:Int,height:Int,screen:CGRect,point:CGPoint)->[Double]?{
        guard width>0,height>0,width<=192,height<=384,bytes.count==width*height*4,screen.width>0,screen.height>0,point.x.isFinite,point.y.isFinite,screen.contains(point) else{return nil}
        let x=min(width-1,max(0,Int((point.x-screen.minX)/screen.width*Double(width))))
        let y=min(height-1,max(0,Int((screen.maxY-point.y)/screen.height*Double(height))))
        let at=(y*width+x)*4
        return [bytes[at+2],bytes[at+1],bytes[at]].map{(Double($0)/255*10000).rounded()/10000}
    }
    static func rgb(_ color:CGColor)->[Double]?{guard let color=NSColor(cgColor:color)?.usingColorSpace(.sRGB) else{return nil};return [color.redComponent,color.greenComponent,color.blueComponent,color.alphaComponent].map{max(0,min(1,$0))}}
    static func over(_ front:[Double],_ back:[Double])->[Double]{let a=max(0,min(1,front[3]));return (0..<3).map{front[$0]*a+back[$0]*(1-a)}}
    static func gradient(_ layer:CAGradientLayer,at point:CGPoint)->[Double]?{
        guard !layer.isHidden,layer.frame.contains(point),let colors=layer.colors as? [CGColor],!colors.isEmpty,layer.frame.height>0 else{return nil}
        let p=max(0,min(1,(point.y-layer.frame.minY)/layer.frame.height))
        let locations=layer.locations?.map(\.doubleValue) ?? colors.indices.map{Double($0)/Double(max(1,colors.count-1))}
        let high=locations.firstIndex(where:{$0>=p}) ?? colors.count-1,low=max(0,high-1)
        guard let a=rgb(colors[low]),let b=rgb(colors[high]) else{return nil}
        let t=locations[high]>locations[low] ? (p-locations[low])/(locations[high]-locations[low]):0
        var result=(0..<4).map{a[$0]+(b[$0]-a[$0])*t};result[3]*=Double(layer.opacity);return result
    }
    static func luminance(_ rgb:[Double])->Double{func linear(_ c:Double)->Double{let c=max(0,min(1,c));return c<=0.04045 ? c/12.92:pow((c+0.055)/1.055,2.4)};return 0.2126*linear(rgb[0])+0.7152*linear(rgb[1])+0.0722*linear(rgb[2])}
    static func contrast(_ a:Double,_ b:Double)->Double{(max(a,b)+0.05)/(min(a,b)+0.05)}
}
