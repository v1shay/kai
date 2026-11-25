import Foundation

/// Dismissal hides presentation, never answers or cancels the pending request.
/// A new request may surface; repeated packets for the dismissed one may not.
struct ApprovalDismissal {
    private var owner:String?,token:String?
    mutating func dismiss(_ request:CompactApprovalRequest){owner=request.thread;token=request.id}
    func allows(_ request:CompactApprovalRequest)->Bool{owner != request.thread || token != request.id}
    mutating func observe(_ request:CompactApprovalRequest?){guard let request else{reset();return};if allows(request){reset()}}
    mutating func reset(){owner=nil;token=nil}
}
