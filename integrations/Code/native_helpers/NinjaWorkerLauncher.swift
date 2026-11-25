import Foundation
import CoreGraphics
import Darwin

// Keep this signed host bundle unchanged during runtime updates. macOS grants
// capture access to its stable identity; the replaceable runtime lives outside it.
let home=FileManager.default.homeDirectoryForCurrentUser
let runtime=home.appendingPathComponent(".kai-ninja/Runtime.app/Contents/MacOS/kai-unified")
if CommandLine.arguments.contains("--permissions-status") {
    var count:UInt32=0
    let result=CGGetActiveDisplayList(0,nil,&count)
    let value:[String:Any]=["screenRecording":CGPreflightScreenCaptureAccess(),"activeDisplays":count,"displayStatus":result.rawValue,"hostBundle":Bundle.main.bundlePath]
    print(String(data:try JSONSerialization.data(withJSONObject:value),encoding:.utf8)!)
    exit(0)
}
guard FileManager.default.isExecutableFile(atPath:runtime.path) else {
    fputs("Kai Ninja runtime is missing; reinstall the host from Kai.\n",stderr);exit(1)
}
let child=Process();child.executableURL=runtime
child.arguments=["--ninja-worker"]+CommandLine.arguments.dropFirst().filter{$0 != "--ninja-worker"}
child.environment=ProcessInfo.processInfo.environment.merging(["KAI_NINJA_MANAGED_HOST":"1"]){_,new in new}
// Keep the authorized process alive as the runtime's responsible parent.
var signals=[DispatchSourceSignal]()
for number in [SIGTERM,SIGINT,SIGHUP] {
    signal(number,SIG_IGN)
    let source=DispatchSource.makeSignalSource(signal:number,queue:.global())
    source.setEventHandler{if child.isRunning{kill(child.processIdentifier,number)}}
    source.resume();signals.append(source)
}
do {try child.run();child.waitUntilExit();exit(child.terminationStatus)}
catch {fputs("Could not start Kai Ninja runtime: \(error.localizedDescription)\n",stderr);exit(1)}
