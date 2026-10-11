use agentix_harness::session::{TurnMachine, TurnState as S, TurnStep as E};
use anyhow::Result;

fn generating(budget: usize) -> Result<TurnMachine> {
    let mut machine = TurnMachine::new(budget);
    machine.step(E::Begin)?;
    machine.step(E::Prepared { compact: false })?;
    Ok(machine)
}

fn batch(machine: &mut TurnMachine, ids: &[&str]) -> Result<()> {
    machine.step(E::ToolBatch(ids.iter().map(|id| id.to_string()).collect()))?;
    machine.step(E::CallsValidated)?;
    Ok(())
}

#[test]
fn budget_requires_matching_results_before_single_finalization() -> Result<()> {
    let mut machine = generating(1)?;
    batch(&mut machine, &["a", "b"])?;
    assert!(machine.step(E::Finish).is_err());
    machine.step(E::StartTool("a".into()))?;
    assert!(machine.step(E::ToolSkipped("a".into())).is_err());
    machine.step(E::ToolResolved("a".into()))?;
    assert!(machine.step(E::StartTool("b".into())).is_err());
    assert!(machine.step(E::Evaluate { stagnant: false }).is_err());
    machine.step(E::ToolSkipped("b".into()))?;
    assert_eq!(
        machine.step(E::Evaluate { stagnant: false })?,
        S::Finalizing
    );
    assert!(machine.step(E::ToolBatch(vec!["c".into()])).is_err());
    machine.step(E::Finish)?;
    assert!(machine.step(E::Finish).is_err());
    assert_eq!(machine.used(), 1);
    Ok(())
}

#[test]
fn invalid_batches_and_out_of_order_results_do_not_change_state() -> Result<()> {
    let mut machine = generating(4)?;
    for ids in [vec![], vec!["".into()], vec!["a".into(), "a".into()]] {
        assert!(machine.step(E::ToolBatch(ids)).is_err());
        assert_eq!(machine.state(), S::Generating);
    }
    batch(&mut machine, &["a"])?;
    assert!(machine.step(E::StartTool("b".into())).is_err());
    assert_eq!(machine.used(), 0);
    machine.step(E::StartTool("a".into()))?;
    assert!(machine.step(E::ToolResolved("b".into())).is_err());
    machine.step(E::ToolResolved("a".into()))?;
    machine.step(E::Evaluate { stagnant: false })?;
    assert!(machine.step(E::ToolBatch(vec!["a".into()])).is_err());
    assert_eq!(machine.state(), S::Generating);
    Ok(())
}

#[test]
fn denied_approval_resolves_call_without_execution() -> Result<()> {
    let mut machine = generating(2)?;
    batch(&mut machine, &["a", "b"])?;
    machine.step(E::NeedApproval)?;
    assert!(machine.step(E::StartTool("a".into())).is_err());
    assert!(machine.step(E::Resume).is_err());
    machine.step(E::ToolSkipped("a".into()))?;
    machine.step(E::NeedApproval)?;
    machine.step(E::ApprovalGranted)?;
    machine.step(E::StartTool("b".into()))?;
    machine.step(E::ToolResolved("b".into()))?;
    assert_eq!(machine.used(), 1);
    Ok(())
}

#[test]
fn cancellation_waits_for_active_tool_and_resolves_pending_calls() -> Result<()> {
    let mut machine = generating(2)?;
    batch(&mut machine, &["a", "b"])?;
    machine.step(E::StartTool("a".into()))?;
    machine.step(E::Cancel)?;
    assert!(machine.step(E::Stopped).is_err());
    assert!(machine.step(E::StartTool("b".into())).is_err());
    machine.step(E::ToolResolved("a".into()))?;
    assert!(machine.step(E::Stopped).is_err());
    machine.step(E::ToolSkipped("b".into()))?;
    assert_eq!(machine.step(E::Stopped)?, S::Cancelled);
    machine.step(E::Reset)?;
    assert_eq!(machine.state(), S::Idle);
    assert_eq!(machine.used(), 0);
    Ok(())
}

#[test]
fn stagnation_requires_explicit_resume_or_finalization() -> Result<()> {
    let mut machine = generating(2)?;
    batch(&mut machine, &["a"])?;
    machine.step(E::StartTool("a".into()))?;
    machine.step(E::ToolResolved("a".into()))?;
    assert_eq!(
        machine.step(E::Evaluate { stagnant: true })?,
        S::AwaitingApproval
    );
    assert!(machine.step(E::ApprovalGranted).is_err());
    assert!(machine.step(E::ToolBatch(vec!["b".into()])).is_err());
    machine.step(E::Resume)?;
    batch(&mut machine, &["b"])?;
    machine.step(E::ToolSkipped("b".into()))?;
    machine.step(E::Evaluate { stagnant: true })?;
    assert_eq!(machine.step(E::Finalize)?, S::Finalizing);
    Ok(())
}

#[test]
fn every_state_rejects_restart_except_idle() -> Result<()> {
    let paths = vec![
        (S::Idle, vec![]),
        (S::Preparing, vec![E::Begin]),
        (S::Compacting, vec![E::Begin, E::Prepared { compact: true }]),
        (
            S::Generating,
            vec![E::Begin, E::Prepared { compact: false }],
        ),
        (
            S::ValidatingCalls,
            vec![
                E::Begin,
                E::Prepared { compact: false },
                E::ToolBatch(vec!["a".into()]),
            ],
        ),
        (
            S::ExecutingTool,
            vec![
                E::Begin,
                E::Prepared { compact: false },
                E::ToolBatch(vec!["a".into()]),
                E::CallsValidated,
            ],
        ),
        (
            S::AwaitingApproval,
            vec![
                E::Begin,
                E::Prepared { compact: false },
                E::ToolBatch(vec!["a".into()]),
                E::CallsValidated,
                E::NeedApproval,
            ],
        ),
        (
            S::EvaluatingProgress,
            vec![
                E::Begin,
                E::Prepared { compact: false },
                E::ToolBatch(vec!["a".into()]),
                E::CallsValidated,
                E::ToolSkipped("a".into()),
            ],
        ),
        (
            S::Finalizing,
            vec![
                E::Begin,
                E::Prepared { compact: false },
                E::ToolBatch(vec!["a".into()]),
                E::CallsValidated,
                E::ToolSkipped("a".into()),
                E::Evaluate { stagnant: true },
                E::Finalize,
            ],
        ),
        (S::Cancelling, vec![E::Begin, E::Cancel]),
        (S::Cancelled, vec![E::Begin, E::Cancel, E::Stopped]),
        (S::Failed, vec![E::Begin, E::Fail]),
        (
            S::Completed,
            vec![E::Begin, E::Prepared { compact: false }, E::Finish],
        ),
    ];
    for (state, path) in paths {
        let mut machine = TurnMachine::new(1);
        for step in path {
            machine.step(step)?;
        }
        assert_eq!(machine.state(), state);
        assert_eq!(machine.step(E::Begin).is_ok(), state == S::Idle);
    }
    let mut zero_budget = TurnMachine::new(0);
    zero_budget.step(E::Begin)?;
    zero_budget.step(E::Prepared { compact: true })?;
    assert_eq!(zero_budget.step(E::Compacted)?, S::Finalizing);
    Ok(())
}
