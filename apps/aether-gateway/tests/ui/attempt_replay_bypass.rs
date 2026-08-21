#[path = "../../src/execution_runtime/attempt_replay.rs"]
mod attempt_replay;

mod sibling_adapter {
    use super::attempt_replay::{
        AttemptDispatchLifecycle, LogicalRequestReplayOwner, ReplayPolicyApproval,
    };

    pub fn bypass() {
        let mut owner = LogicalRequestReplayOwner::new_disabled();
        let (attempt, _) = owner.begin_first_attempt().unwrap();
        let _approval = ReplayPolicyApproval::approve_disabled(&attempt).unwrap();
        let _fresh = AttemptDispatchLifecycle::default();
    }
}

fn main() {
    sibling_adapter::bypass();
}
