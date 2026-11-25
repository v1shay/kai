import Foundation

enum TaskPetAssignments {
    /// Reserve existing pets before filling gaps, repairing collisions from resumed chats.
    static func reconcile(_ assignments:[String:String],taskIDs:[String],available:[String],preferred:String,explicit:Set<String>,priority:String?=nil)->[String:String]{
        var result=assignments,used=Set<String>(),pending=[String](),seen=Set<String>()
        let ordered=taskIDs.filter{seen.insert($0).inserted}.sorted {
            let lhs=$0 == priority ? 2:(explicit.contains($0) ? 1:0)
            let rhs=$1 == priority ? 2:(explicit.contains($1) ? 1:0)
            return lhs>rhs
        }
        for id in ordered {
            if let pet=assignments[id],!pet.isEmpty,(available.contains(pet) || explicit.contains(id)),used.insert(pet).inserted {
                result[id]=pet
            } else {pending.append(id)}
        }
        let pool=available.isEmpty ? [preferred]:available
        for id in pending {
            let free=pool.filter{!used.contains($0)}
            let pet=free.contains(preferred) ? preferred:(free.first ?? pool.min { a,b in
                ordered.filter{result[$0] == a}.count < ordered.filter{result[$0] == b}.count
            } ?? preferred)
            result[id]=pet;used.insert(pet)
        }
        return result
    }

    static func cycleChoices(for id:String,assignments:[String:String],taskIDs:[String],available:[String])->[String]{
        let others=Set(taskIDs.filter{$0 != id}.compactMap{assignments[$0]})
        let choices=available.filter{$0 != assignments[id]}
        let free=choices.filter{!others.contains($0)}
        return free.isEmpty ? choices:free
    }
}
