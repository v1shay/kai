import AppKit

private final class RenderingProbePanel:NSPanel {
    var probeOcclusion:NSWindow.OcclusionState=[.visible]
    override var occlusionState:NSWindow.OcclusionState{probeOcclusion}
}

@main struct NativeRenderingHarness {
    @MainActor static func main(){
        _=NSApplication.shared
        let panel=RenderingProbePanel(contentRect:CGRect(x:-3000,y:-3000,width:1000,height:500),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        panel.isReleasedWhenClosed=false;panel.orderFrontRegardless()
        defer{panel.close()}
        let scene=NinjaAtmosphereView(frame:panel.contentView!.bounds)
        panel.contentView!.addSubview(scene)
        for width in stride(from:600,through:1000,by:20){
            scene.frame.size.width=CGFloat(width)
            scene.configure(identity:"rendering",style:"ice",working:false,motion:false,visible:true)
        }
        let grid=scene.layer!.sublayers!.first!.sublayers!.first!
        func gridImage()->CGImage?{guard let contents=grid.contents else{return nil};return (contents as! CGImage)}
        let deadline=Date().addingTimeInterval(10)
        let expectedWidth=Int(scene.bounds.width*panel.backingScaleFactor)
        while gridImage()?.width != expectedWidth && Date()<deadline{
            RunLoop.main.run(until:Date().addingTimeInterval(0.01))
        }
        precondition(gridImage()?.width==expectedWidth,"Coalesced raster must eventually match the latest geometry")
        precondition(scene.diagnosticRasterCacheCount<=4 && scene.diagnosticRasterCacheBytes<=16*1024*1024,"Raster cache must enforce hard entry and byte limits")
        let cached=grid.contents as! CGImage
        let jobsBefore=scene.diagnosticRasterJobsStarted
        scene.frame.size.width=700
        scene.configure(identity:"rendering",style:"fire",working:false,motion:false,visible:true)
        scene.frame.size.width=800
        scene.configure(identity:"rendering",style:"water",working:false,motion:false,visible:true)
        scene.frame.size.width=1000
        scene.configure(identity:"rendering",style:"ice",working:false,motion:false,visible:true)
        precondition(gridImage() === cached,"Returning to a cached grid must not synchronously rerasterize")
        let cacheDeadline=Date().addingTimeInterval(10)
        while scene.diagnosticRasterPending && Date()<cacheDeadline{RunLoop.main.run(until:Date().addingTimeInterval(0.01))}
        precondition(!scene.diagnosticRasterPending && scene.diagnosticRasterJobsStarted==jobsBefore+1,"Cache return must discard the now-stale queued raster")
        for size in [CGSize(width:20000,height:210),CGSize(width:32768,height:210)]{
            let raster=NinjaAtmosphereView.gridRasterGeometry(size:size,scale:2)!
            precondition(raster.width<=8192 && raster.height<=8192 && raster.width*raster.height*4<=16*1024*1024,"Extreme geometry must bound bitmap allocation")
        }
        precondition(NinjaAtmosphereView.gridRasterGeometry(size:CGSize(width:1000,height:210),scale:2)!.scale==2,"Normal windows must preserve exact backing resolution")
        for width in [CGFloat.nan,CGFloat.infinity,0,1e100]{precondition(NinjaAtmosphereView.gridRasterGeometry(size:CGSize(width:width,height:210),scale:2)==nil)}
        precondition(gridImage() === cached,"An older in-flight raster must not replace the latest style/geometry")
        scene.configure(identity:"rendering",style:"ice",working:true,motion:true,visible:true)
        precondition(scene.layer!.speed==1)
        let epoch=scene.diagnosticEpoch
        panel.probeOcclusion=[]
        NotificationCenter.default.post(name:NSWindow.didChangeOcclusionStateNotification,object:panel)
        precondition(scene.layer!.speed==0,"Occluded atmosphere must pause compositor playback")
        panel.probeOcclusion=[.visible]
        NotificationCenter.default.post(name:NSWindow.didChangeOcclusionStateNotification,object:panel)
        precondition(scene.layer!.speed==1 && scene.diagnosticEpoch==epoch,"Uncovering must resume the same animation timeline")
        for width in [1200,1300,1400,1500,1600,1700,3800,3900]{
            scene.frame.size.width=CGFloat(width)
            scene.configure(identity:"cache-budget",style:"ice",working:false,motion:false,visible:true)
            let settle=Date().addingTimeInterval(10)
            while scene.diagnosticRasterPending && Date()<settle{RunLoop.main.run(until:Date().addingTimeInterval(0.01))}
            precondition(!scene.diagnosticRasterPending)
            precondition(scene.diagnosticRasterCacheCount<=4 && scene.diagnosticRasterCacheBytes<=16*1024*1024,"Cache must evict across repeated large raster insertions")
        }
        scene.suspend();precondition(scene.layer!.speed==0)
        print("NATIVE_RENDERING_OK latest-size raster coalescing, stale queued-work cancellation, bounded geometry/memory, cache reuse, occlusion pause/resume, timeline retention")
    }
}
