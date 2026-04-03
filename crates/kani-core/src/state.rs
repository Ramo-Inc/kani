use kani_proto::event::HostId;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum CursorState {
    Idle {
        active: HostId,
    },
    Transitioning {
        from: HostId,
        to: HostId,
        transition_id: u64,
    },
}

impl std::fmt::Display for CursorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle { active } => {
                let s = active.to_string();
                write!(f, "Idle({})", &s[..8])
            }
            Self::Transitioning {
                from,
                to,
                transition_id,
            } => {
                let from_s = from.to_string();
                let to_s = to.to_string();
                write!(
                    f,
                    "Transitioning({}→{}, tid={})",
                    &from_s[..8],
                    &to_s[..8],
                    transition_id
                )
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("transition blocked: already transitioning from {from} to {to}")]
    TransitionBlocked { from: HostId, to: HostId },
}

pub struct CursorStateMachine {
    state: CursorState,
    #[allow(dead_code)]
    server_id: HostId,
    next_transition_id: u64,
}

impl CursorStateMachine {
    pub fn new(server_id: HostId) -> Self {
        Self {
            state: CursorState::Idle { active: server_id },
            server_id,
            next_transition_id: 1,
        }
    }

    pub fn state(&self) -> &CursorState {
        &self.state
    }

    pub fn active_host(&self) -> HostId {
        match &self.state {
            CursorState::Idle { active } => *active,
            CursorState::Transitioning { from, .. } => *from,
        }
    }

    /// Begin transition. Returns transition_id for the Enter message.
    pub fn begin_transition(&mut self, target: HostId) -> Result<u64, StateError> {
        match &self.state {
            CursorState::Idle { active } => {
                let from = *active;
                let tid = self.next_transition_id;
                self.next_transition_id += 1;
                self.state = CursorState::Transitioning {
                    from,
                    to: target,
                    transition_id: tid,
                };
                Ok(tid)
            }
            CursorState::Transitioning { from, to, .. } => Err(StateError::TransitionBlocked {
                from: *from,
                to: *to,
            }),
        }
    }

    /// Process Ack. Only completes if host AND transition_id match.
    /// Stale/duplicate Acks silently ignored.
    pub fn ack_received(&mut self, from_host: HostId, transition_id: u64) {
        if let CursorState::Transitioning {
            to,
            transition_id: tid,
            ..
        } = &self.state
        {
            if *to == from_host && *tid == transition_id {
                self.state = CursorState::Idle { active: from_host };
            }
        }
    }

    pub fn timeout(&mut self) {
        if let CursorState::Transitioning { from, .. } = &self.state {
            let revert = *from;
            self.state = CursorState::Idle { active: revert };
        }
    }

    pub fn emergency_reset(&mut self, to_host: HostId) {
        self.state = CursorState::Idle { active: to_host };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn server() -> HostId {
        Uuid::from_bytes([0x01; 16])
    }

    fn host_b() -> HostId {
        Uuid::from_bytes([0x02; 16])
    }

    fn host_c() -> HostId {
        Uuid::from_bytes([0x03; 16])
    }

    #[test]
    fn test_initial_state_idle() {
        let sm = CursorStateMachine::new(server());
        assert_eq!(sm.state(), &CursorState::Idle { active: server() });
        assert_eq!(sm.active_host(), server());
    }

    #[test]
    fn test_begin_transition_returns_id() {
        let mut sm = CursorStateMachine::new(server());
        let tid = sm.begin_transition(host_b()).unwrap();
        assert_eq!(tid, 1);
        assert_eq!(
            sm.state(),
            &CursorState::Transitioning {
                from: server(),
                to: host_b(),
                transition_id: 1,
            }
        );
    }

    #[test]
    fn test_transition_blocked() {
        let mut sm = CursorStateMachine::new(server());
        sm.begin_transition(host_b()).unwrap();
        let result = sm.begin_transition(host_c());
        assert!(result.is_err());
    }

    #[test]
    fn test_ack_completes_transition() {
        let mut sm = CursorStateMachine::new(server());
        let tid = sm.begin_transition(host_b()).unwrap();
        sm.ack_received(host_b(), tid);
        assert_eq!(sm.state(), &CursorState::Idle { active: host_b() });
    }

    #[test]
    fn test_ack_wrong_transition_id_ignored() {
        let mut sm = CursorStateMachine::new(server());
        let _tid = sm.begin_transition(host_b()).unwrap();
        sm.ack_received(host_b(), 999);
        // Should still be transitioning
        assert!(matches!(sm.state(), CursorState::Transitioning { .. }));
    }

    #[test]
    fn test_stale_ack_after_timeout_ignored() {
        let mut sm = CursorStateMachine::new(server());
        let tid = sm.begin_transition(host_b()).unwrap();
        sm.timeout();
        assert_eq!(sm.state(), &CursorState::Idle { active: server() });
        // Late ack arrives
        sm.ack_received(host_b(), tid);
        // Should still be idle on server
        assert_eq!(sm.state(), &CursorState::Idle { active: server() });
    }

    #[test]
    fn test_timeout_reverts() {
        let mut sm = CursorStateMachine::new(server());
        sm.begin_transition(host_b()).unwrap();
        sm.timeout();
        assert_eq!(sm.state(), &CursorState::Idle { active: server() });
    }

    #[test]
    fn test_emergency_reset() {
        let mut sm = CursorStateMachine::new(server());
        sm.begin_transition(host_b()).unwrap();
        sm.emergency_reset(host_c());
        assert_eq!(sm.state(), &CursorState::Idle { active: host_c() });
    }

    #[test]
    fn test_n_host_chain_a_b_c() {
        let mut sm = CursorStateMachine::new(server());

        // server -> B
        let tid1 = sm.begin_transition(host_b()).unwrap();
        sm.ack_received(host_b(), tid1);
        assert_eq!(sm.state(), &CursorState::Idle { active: host_b() });

        // B -> C
        let tid2 = sm.begin_transition(host_c()).unwrap();
        assert_ne!(tid1, tid2);
        sm.ack_received(host_c(), tid2);
        assert_eq!(sm.state(), &CursorState::Idle { active: host_c() });
    }

    #[test]
    fn test_cursor_state_display_idle() {
        let sm = CursorStateMachine::new(server());
        let s = format!("{}", sm.state());
        assert!(s.starts_with("Idle("));
    }

    #[test]
    fn test_cursor_state_display_transitioning() {
        let mut sm = CursorStateMachine::new(server());
        sm.begin_transition(host_b()).unwrap();
        let s = format!("{}", sm.state());
        assert!(s.starts_with("Transitioning("));
        assert!(s.contains("tid="));
    }

    #[test]
    fn test_transition_ids_are_unique() {
        let mut sm = CursorStateMachine::new(server());

        let tid1 = sm.begin_transition(host_b()).unwrap();
        sm.ack_received(host_b(), tid1);

        let tid2 = sm.begin_transition(host_c()).unwrap();
        sm.ack_received(host_c(), tid2);

        let tid3 = sm.begin_transition(server()).unwrap();

        assert_ne!(tid1, tid2);
        assert_ne!(tid2, tid3);
        assert_ne!(tid1, tid3);
    }
}
