import XCTest
import AppKit
@testable import NotchPrototype
final class AccentCornerTests:XCTestCase {
    @MainActor func testApprovalAndFullExpansionCarryVisibleGradientsToTheirBottomCorners()throws{
        let recipe=GradientRecipe(angleDegrees:0,cycleDurationMs:1000,stops:[GradientStop(location:0,color:"#00ff00")])
        let profile=PetGradientProfile(palette:PetPalette(shadow:"#000000",primary:"#00ff00",secondary:"#00ff00",accent:"#00ff00",highlight:"#ffffff",foreground:"#ffffff"),gradients:PetGradients(ambient:recipe,thinking:recipe,working:recipe,success:recipe,warning:recipe,error:recipe))
        for height:CGFloat in [CompactApprovalRequest.height(notch:39),320]{
            let indicator=NotchIndicatorEngine(),root=CALayer(),mask=CAShapeLayer()
            root.frame=CGRect(x:0,y:0,width:470,height:320);root.backgroundColor=NSColor.black.cgColor
            mask.path=NotchSizing.path(canvas:root.bounds.size,width:370,height:height);root.mask=mask
            indicator.leftAccentLayer.frame=CGRect(x:43,y:281,width:115,height:82)
            indicator.accentLayer.frame=CGRect(x:312,y:281,width:115,height:82)
            root.addSublayer(indicator.leftAccentLayer);root.addSublayer(indicator.accentLayer)
            indicator.layout();indicator.setProfile(profile,wave:false);indicator.setAccentVisibility(left:true,right:true)
            indicator.setAccentExpanded(false,duration:0);indicator.layout()
            indicator.moveAccents(toBottom:320-height,expanded:true,duration:0.28)
            for layer in [indicator.leftAccentLayer,indicator.accentLayer]{
                let travel=try XCTUnwrap(layer.animation(forKey:"cornerTravel") as? CABasicAnimation)
                XCTAssertEqual(travel.fromValue as? CGFloat,322);XCTAssertEqual(travel.toValue as? CGFloat,361-height)
                let fade=try XCTUnwrap(layer.sublayers?.first?.animation(forKey:"sizeBlend") as? CABasicAnimation)
                XCTAssertEqual(fade.fromValue as? Float,0);XCTAssertEqual(fade.toValue as? Float,1)
            }
            let context=try XCTUnwrap(CGContext(data:nil,width:470,height:320,bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue))
            root.render(in:context);let bitmap=NSBitmapImageRep(cgImage:try XCTUnwrap(context.makeImage()))
            for edge in [50..<90,380..<420]{
                XCTAssertTrue((Int(height)-32..<Int(height)-8).contains{y in edge.contains{x in (bitmap.colorAt(x:x,y:y)?.greenComponent ?? 0)>0.03}},"Gradient must remain visible at both bottom corners, height \(height)")
            }
        }
    }
    @MainActor func testCompactGlowCoversOuterShouldersThroughFullNotchHeight(){
        let indicator=NotchIndicatorEngine()
        for height:CGFloat in [39,48,65]{
            indicator.compactBandHeight=height
            indicator.leftAccentLayer.frame=CGRect(x:0,y:0,width:115,height:82)
            indicator.accentLayer.frame=CGRect(x:0,y:0,width:115,height:82)
            indicator.layout()
            let left=indicator.leftAccentLayer.sublayers!.last!,right=indicator.accentLayer.sublayers!.last!
            XCTAssertEqual(left.frame.minX,0);XCTAssertEqual(right.frame.maxX,115)
            XCTAssertEqual(left.frame.height,height);XCTAssertEqual(right.frame.height,height)
            XCTAssertEqual(left.mask?.contentsGravity,.resize)
        }
    }
}
