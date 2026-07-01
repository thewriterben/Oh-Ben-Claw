//! Saga orchestration — atomic-ish multi-node deployment with rollback.
//!
//! Applying a deployment scheme touches several nodes: flash one, configure
//! another, register a third. If step 3 fails, steps 1–2 have already taken
//! effect — and a half-applied fleet is worse than an unapplied one. The **saga**
//! pattern handles this without distributed transactions: each step pairs a
//! forward action with a **compensating action** that undoes it, and on the first
//! failure the saga unwinds — running the compensations for every completed step
//! in reverse order. Either the whole deployment commits, or it rolls back to where
//! it started (best-effort; compensation failures are reported, not hidden).
//!
//! Adapted from a sibling project's `core/events` Saga; the implementation here is
//! original and dependency-free.

use async_trait::async_trait;

/// One saga step: a forward action and the compensation that undoes it.
#[async_trait]
pub trait SagaAction: Send + Sync {
    /// Apply this step (e.g. flash/configure a node).
    async fn execute(&self) -> anyhow::Result<()>;
    /// Undo this step (called during rollback if a later step fails).
    async fn compensate(&self) -> anyhow::Result<()>;
    /// A short name for logs and outcomes.
    fn name(&self) -> &str;
}

/// The result of running a saga.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SagaOutcome {
    /// Every step executed; the deployment is applied.
    Committed { steps: usize },
    /// A step failed; completed steps were compensated (unwound) in reverse.
    RolledBack {
        /// The step whose `execute` failed.
        failed_at: String,
        /// How many completed steps were successfully compensated.
        compensated: usize,
        /// Steps whose `compensate` *also* failed — manual cleanup may be needed.
        compensation_failures: Vec<String>,
    },
}

impl SagaOutcome {
    /// Whether the deployment fully committed.
    pub fn committed(&self) -> bool {
        matches!(self, SagaOutcome::Committed { .. })
    }
    /// Whether rollback left any step un-compensated (needs attention).
    pub fn clean(&self) -> bool {
        match self {
            SagaOutcome::Committed { .. } => true,
            SagaOutcome::RolledBack { compensation_failures, .. } => compensation_failures.is_empty(),
        }
    }
}

/// A sequence of compensable steps run as a unit.
#[derive(Default)]
pub struct Saga {
    steps: Vec<Box<dyn SagaAction>>,
}

impl Saga {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// Append a step (builder style).
    pub fn step(mut self, action: Box<dyn SagaAction>) -> Self {
        self.steps.push(action);
        self
    }

    /// Append a step in place.
    pub fn add(&mut self, action: Box<dyn SagaAction>) {
        self.steps.push(action);
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Run the saga. Execute steps in order; on the first failure, compensate the
    /// already-completed steps in reverse and return [`SagaOutcome::RolledBack`].
    /// On full success, [`SagaOutcome::Committed`].
    pub async fn run(&self) -> SagaOutcome {
        let mut completed: Vec<&dyn SagaAction> = Vec::new();
        for step in &self.steps {
            match step.execute().await {
                Ok(()) => completed.push(step.as_ref()),
                Err(_) => {
                    let mut compensated = 0;
                    let mut compensation_failures = Vec::new();
                    // Unwind in reverse: last-applied is undone first.
                    for done in completed.iter().rev() {
                        match done.compensate().await {
                            Ok(()) => compensated += 1,
                            Err(_) => compensation_failures.push(done.name().to_string()),
                        }
                    }
                    return SagaOutcome::RolledBack {
                        failed_at: step.name().to_string(),
                        compensated,
                        compensation_failures,
                    };
                }
            }
        }
        SagaOutcome::Committed { steps: self.steps.len() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A step that records its execute/compensate calls to a shared log, and can be
    /// scripted to fail on either.
    struct RecordingStep {
        name: String,
        fail_execute: bool,
        fail_compensate: bool,
        log: Arc<Mutex<Vec<String>>>,
    }
    impl RecordingStep {
        fn ok(name: &str, log: Arc<Mutex<Vec<String>>>) -> Box<dyn SagaAction> {
            Box::new(Self { name: name.into(), fail_execute: false, fail_compensate: false, log })
        }
        fn failing(name: &str, log: Arc<Mutex<Vec<String>>>) -> Box<dyn SagaAction> {
            Box::new(Self { name: name.into(), fail_execute: true, fail_compensate: false, log })
        }
        fn bad_compensate(name: &str, log: Arc<Mutex<Vec<String>>>) -> Box<dyn SagaAction> {
            Box::new(Self { name: name.into(), fail_execute: false, fail_compensate: true, log })
        }
    }
    #[async_trait]
    impl SagaAction for RecordingStep {
        async fn execute(&self) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("exec:{}", self.name));
            if self.fail_execute {
                anyhow::bail!("execute failed");
            }
            Ok(())
        }
        async fn compensate(&self) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("comp:{}", self.name));
            if self.fail_compensate {
                anyhow::bail!("compensate failed");
            }
            Ok(())
        }
        fn name(&self) -> &str {
            &self.name
        }
    }

    #[tokio::test]
    async fn all_steps_succeed_commits() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let saga = Saga::new()
            .step(RecordingStep::ok("a", Arc::clone(&log)))
            .step(RecordingStep::ok("b", Arc::clone(&log)));
        let out = saga.run().await;
        assert_eq!(out, SagaOutcome::Committed { steps: 2 });
        assert!(out.committed() && out.clean());
        assert_eq!(*log.lock().unwrap(), vec!["exec:a", "exec:b"]);
    }

    #[tokio::test]
    async fn a_failure_unwinds_completed_steps_in_reverse() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let saga = Saga::new()
            .step(RecordingStep::ok("a", Arc::clone(&log)))
            .step(RecordingStep::ok("b", Arc::clone(&log)))
            .step(RecordingStep::failing("c", Arc::clone(&log)));
        let out = saga.run().await;
        assert_eq!(
            out,
            SagaOutcome::RolledBack {
                failed_at: "c".into(),
                compensated: 2,
                compensation_failures: vec![],
            }
        );
        // a, b applied; c failed; then b, a compensated (reverse order)
        assert_eq!(
            *log.lock().unwrap(),
            vec!["exec:a", "exec:b", "exec:c", "comp:b", "comp:a"]
        );
    }

    #[tokio::test]
    async fn a_failed_compensation_is_reported_not_hidden() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let saga = Saga::new()
            .step(RecordingStep::bad_compensate("a", Arc::clone(&log)))
            .step(RecordingStep::failing("b", Arc::clone(&log)));
        let out = saga.run().await;
        assert!(!out.clean(), "a failed compensation makes the rollback dirty");
        match out {
            SagaOutcome::RolledBack { failed_at, compensated, compensation_failures } => {
                assert_eq!(failed_at, "b");
                assert_eq!(compensated, 0);
                assert_eq!(compensation_failures, vec!["a".to_string()]);
            }
            other => panic!("expected rollback, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_first_step_failing_compensates_nothing() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let saga = Saga::new().step(RecordingStep::failing("a", Arc::clone(&log)));
        let out = saga.run().await;
        assert_eq!(
            out,
            SagaOutcome::RolledBack { failed_at: "a".into(), compensated: 0, compensation_failures: vec![] }
        );
        assert_eq!(*log.lock().unwrap(), vec!["exec:a"]);
    }
}
