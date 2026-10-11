use std::collections::{HashSet, VecDeque};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Idle,
    Preparing,
    Compacting,
    Generating,
    ValidatingCalls,
    AwaitingApproval,
    ExecutingTool,
    EvaluatingProgress,
    Finalizing,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl TurnState {
    pub fn is_active(self) -> bool {
        !matches!(
            self,
            Self::Idle | Self::Completed | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone)]
pub enum TurnStep {
    Begin,
    Prepared { compact: bool },
    Compacted,
    ToolBatch(Vec<String>),
    CallsValidated,
    NeedApproval,
    ApprovalGranted,
    StartTool(String),
    ToolResolved(String),
    ToolSkipped(String),
    Evaluate { stagnant: bool },
    Resume,
    Finalize,
    Finish,
    Cancel,
    Stopped,
    Fail,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionError {
    pub state: TurnState,
    pub reason: &'static str,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid turn transition from {:?}: {}",
            self.state, self.reason
        )
    }
}

impl std::error::Error for TransitionError {}

#[derive(Debug)]
pub struct TurnMachine {
    state: TurnState,
    budget: usize,
    used: usize,
    pending: VecDeque<String>,
    seen: HashSet<String>,
    active_tool: Option<String>,
    intervention: bool,
}

impl TurnMachine {
    pub fn new(budget: usize) -> Self {
        Self {
            state: TurnState::Idle,
            budget,
            used: 0,
            pending: VecDeque::new(),
            seen: HashSet::new(),
            active_tool: None,
            intervention: false,
        }
    }

    pub fn state(&self) -> TurnState {
        self.state
    }
    pub fn used(&self) -> usize {
        self.used
    }
    pub fn remaining(&self) -> usize {
        self.budget.saturating_sub(self.used)
    }
    pub fn next_call(&self) -> Option<&str> {
        self.pending.front().map(String::as_str)
    }

    fn error(&self, reason: &'static str) -> TransitionError {
        TransitionError {
            state: self.state,
            reason,
        }
    }

    pub fn step(&mut self, step: TurnStep) -> Result<TurnState, TransitionError> {
        use TurnState as S;
        use TurnStep as E;
        let next = match (self.state, step) {
            (S::Idle, E::Begin) => S::Preparing,
            (S::Preparing, E::Prepared { compact: true }) => S::Compacting,
            (S::Preparing, E::Prepared { compact: false }) | (S::Compacting, E::Compacted) => {
                if self.budget == 0 {
                    S::Finalizing
                } else {
                    S::Generating
                }
            }
            (S::Generating, E::ToolBatch(ids)) => {
                let unique: HashSet<&str> = ids.iter().map(String::as_str).collect();
                if ids.is_empty()
                    || ids.len() > 1000
                    || unique.len() != ids.len()
                    || ids
                        .iter()
                        .any(|id| id.trim().is_empty() || id.len() > 128 || self.seen.contains(id))
                {
                    return Err(self.error("empty, duplicate, reused, or oversized tool call IDs"));
                }
                self.seen.extend(ids.iter().cloned());
                self.pending = ids.into();
                S::ValidatingCalls
            }
            (S::ValidatingCalls, E::CallsValidated) => S::ExecutingTool,
            (S::ExecutingTool, E::NeedApproval) if self.active_tool.is_none() => {
                self.intervention = false;
                S::AwaitingApproval
            }
            (S::AwaitingApproval, E::ApprovalGranted) if !self.intervention => S::ExecutingTool,
            (S::ExecutingTool, E::StartTool(id)) => {
                if self.active_tool.is_some()
                    || self.next_call() != Some(id.as_str())
                    || self.remaining() == 0
                {
                    return Err(self.error("tool is out of order, already active, or over budget"));
                }
                self.used += 1;
                self.active_tool = Some(id);
                S::ExecutingTool
            }
            (S::ExecutingTool | S::Cancelling, E::ToolResolved(id)) => {
                if self.active_tool.as_deref() != Some(id.as_str()) {
                    return Err(self.error("result does not match the active tool"));
                }
                self.active_tool = None;
                self.pending.pop_front();
                self.after_resolution()
            }
            (S::ExecutingTool | S::AwaitingApproval | S::Cancelling, E::ToolSkipped(id)) => {
                if self.active_tool.is_some() || self.next_call() != Some(id.as_str()) {
                    return Err(self.error("cannot skip an active or out-of-order tool"));
                }
                self.pending.pop_front();
                self.after_resolution()
            }
            (S::EvaluatingProgress, E::Evaluate { stagnant }) => {
                if self.remaining() == 0 {
                    S::Finalizing
                } else if stagnant {
                    self.intervention = true;
                    S::AwaitingApproval
                } else {
                    S::Generating
                }
            }
            (S::AwaitingApproval, E::Resume) if self.intervention => {
                self.intervention = false;
                S::Generating
            }
            (S::AwaitingApproval, E::Finalize) if self.intervention => S::Finalizing,
            (S::Generating | S::Finalizing, E::Finish) if self.pending.is_empty() => S::Completed,
            (state, E::Cancel) if state.is_active() && state != S::Cancelling => S::Cancelling,
            (S::Cancelling, E::Stopped)
                if self.active_tool.is_none() && self.pending.is_empty() =>
            {
                S::Cancelled
            }
            (state, E::Fail) if state.is_active() => S::Failed,
            (S::Completed | S::Cancelled | S::Failed, E::Reset) => {
                self.used = 0;
                self.pending.clear();
                self.seen.clear();
                self.active_tool = None;
                self.intervention = false;
                S::Idle
            }
            _ => return Err(self.error("step is not allowed in this state")),
        };
        self.state = next;
        Ok(next)
    }

    fn after_resolution(&self) -> TurnState {
        if self.state == TurnState::Cancelling {
            TurnState::Cancelling
        } else if self.pending.is_empty() {
            TurnState::EvaluatingProgress
        } else {
            TurnState::ExecutingTool
        }
    }
}
