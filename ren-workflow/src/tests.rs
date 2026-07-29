use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::json;

use crate::{
    AgentOptions, AgentRequest, AgentResult, Capability, EchoHost, Engine, Host, HostError,
    Journal, JournalEntry, ParallelSlot, RunOptions, WorkflowError,
    registry::{self, WorkflowSource},
    schema,
};

const META: &str = r#"let meta = #{
    name: "test-workflow",
    description: "A test workflow",
    phases: [#{ title: "Work", detail: "Run test work" }]
};
"#;

#[derive(Clone)]
struct CountingHost {
    calls: Rc<Cell<usize>>,
}

impl Host for CountingHost {
    fn run_agent(
        &self,
        request: &AgentRequest,
    ) -> Result<AgentResult, HostError> {
        self.calls.set(self.calls.get() + 1);
        Ok(AgentResult {
            agent_id: format!("agent-{}", self.calls.get()),
            success: true,
            output: request.prompt.clone(),
            cancelled: false,
            tokens_used: 1,
            duration_ms: 0,
        })
    }
}

#[derive(Clone, Copy)]
enum SecondCallOutcome {
    AgentFailure,
    Cancelled,
    InfrastructureFailure,
}

#[derive(Clone)]
struct SecondCallHost {
    calls: Rc<Cell<usize>>,
    outcome: SecondCallOutcome,
}

impl Host for SecondCallHost {
    fn run_agent(
        &self,
        request: &AgentRequest,
    ) -> Result<AgentResult, HostError> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if call == 2 {
            return match self.outcome {
                SecondCallOutcome::AgentFailure => Ok(AgentResult {
                    agent_id: "failed-agent".into(),
                    success: false,
                    output: "agent refused the task".into(),
                    cancelled: false,
                    tokens_used: 1,
                    duration_ms: 0,
                }),
                SecondCallOutcome::Cancelled => Ok(AgentResult {
                    agent_id: "cancelled-agent".into(),
                    success: false,
                    output: "cancelled".into(),
                    cancelled: true,
                    tokens_used: 1,
                    duration_ms: 0,
                }),
                SecondCallOutcome::InfrastructureFailure => {
                    Err(HostError::new("simulated infrastructure failure"))
                },
            };
        }
        Ok(AgentResult {
            agent_id: format!("agent-{call}"),
            success: true,
            output: request.prompt.clone(),
            cancelled: false,
            tokens_used: 1,
            duration_ms: 0,
        })
    }
}

#[test]
fn simple_workflow_completes_with_expected_value() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
phase("Work");
let result = agent("hello");
log(result.output);
complete(#{{ answer: result.output, ok: result.success }});
"#
    );
    let result = Engine::new(EchoHost).run_script(&script, RunOptions::default())?;

    assert_eq!(
        result.complete,
        Some(json!({"answer": "hello", "ok": true}))
    );
    assert_eq!(result.logs, ["hello"]);
    assert_eq!(result.phases, ["Work"]);
    assert!(result.paused.is_none());
    Ok(())
}

#[test]
fn identical_runs_are_byte_identical() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
let encoded = json_encode(args);
let id = fingerprint(encoded);
let result = agent(id, #{{ label: "stable" }});
complete(#{{ result: result, budget: budget() }});
"#
    );
    let options = RunOptions {
        args: Some(json!({"b": 2, "a": [true, "x"]})),
        ..RunOptions::default()
    };
    let engine = Engine::new(EchoHost);
    let first = engine.run_script(&script, options.clone())?;
    let second = engine.run_script(&script, options)?;

    assert_eq!(serde_json::to_vec(&first)?, serde_json::to_vec(&second)?);
    Ok(())
}

#[test]
fn serialized_journal_replays_without_calling_host() -> Result<(), WorkflowError> {
    let script = format!("{META}\nlet result = agent(\"once\"); complete(result.output);");
    let first_calls = Rc::new(Cell::new(0));
    let first = Engine::new(CountingHost {
        calls: Rc::clone(&first_calls),
    })
    .run_script(&script, RunOptions::default())?;
    assert_eq!(first_calls.get(), 1);

    let serialized = first.journal.to_json()?;
    let replay_calls = Rc::new(Cell::new(0));
    let replay = Engine::new(CountingHost {
        calls: Rc::clone(&replay_calls),
    })
    .run_script(
        &script,
        RunOptions {
            journal: Journal::from_json(&serialized)?,
            ..RunOptions::default()
        },
    )?;

    assert_eq!(replay_calls.get(), 0);
    assert_eq!(first, replay);
    Ok(())
}

#[test]
fn parallel_budget_admission_is_atomic() {
    let script = format!(
        r#"{META}
parallel([
    #{{ prompt: "one", label: "first" }},
    #{{ prompt: "two", label: "second" }}
]);
complete(true);
"#
    );
    let calls = Rc::new(Cell::new(0));
    let result = Engine::new(CountingHost {
        calls: Rc::clone(&calls),
    })
    .run_script(
        &script,
        RunOptions {
            agent_budget: 1,
            ..RunOptions::default()
        },
    );

    assert!(matches!(result, Err(WorkflowError::Runtime(_))));
    assert_eq!(calls.get(), 0);
}

#[test]
fn budget_counts_agent_and_parallel_slots() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
agent("one");
parallel([#{{ prompt: "two" }}, #{{ prompt: "three" }}]);
complete(budget());
"#
    );
    let result = Engine::new(EchoHost).run_script(
        &script,
        RunOptions {
            agent_budget: 3,
            ..RunOptions::default()
        },
    )?;

    assert_eq!(
        result.complete,
        Some(json!({"total": 3, "spent": 3, "reserved": 0, "remaining": 0}))
    );
    Ok(())
}

#[test]
fn nondeterministic_functions_are_rejected() {
    for call in [
        "timestamp()",
        "sleep(1)",
        "rand()",
        "random()",
        "rand_int(1, 4)",
    ] {
        let script = format!("{META}\n{call}; complete(true);");
        let result = Engine::new(EchoHost).run_script(&script, RunOptions::default());
        assert!(
            matches!(result, Err(WorkflowError::Runtime(message)) if message.contains("unavailable in deterministic workflows")),
            "guard did not reject {call}"
        );
    }
}

#[test]
fn extracts_metadata_and_validates_name() -> Result<(), WorkflowError> {
    let engine = Engine::new(EchoHost);
    let workflow = engine.compile(&format!("{META}\ncomplete(true);"))?;
    assert_eq!(workflow.metadata().name, "test-workflow");
    assert_eq!(workflow.metadata().phases[0].title, "Work");

    let invalid = META.replace("test-workflow", "Test_Workflow");
    assert!(matches!(
        engine.compile(&format!("{invalid}\ncomplete(true);")),
        Err(WorkflowError::InvalidMeta(_))
    ));
    let impure = "let meta = make_meta(); complete(true);";
    assert!(matches!(
        engine.compile(impure),
        Err(WorkflowError::InvalidMeta(_))
    ));
    Ok(())
}

fn plain_request(prompt: &str) -> AgentRequest {
    AgentRequest {
        prompt: prompt.to_owned(),
        options: AgentOptions::default(),
    }
}

fn successful_result(
    agent_id: &str,
    output: &str,
) -> AgentResult {
    AgentResult {
        agent_id: agent_id.to_owned(),
        success: true,
        output: output.to_owned(),
        cancelled: false,
        tokens_used: 1,
        duration_ms: 0,
    }
}

#[test]
fn caught_divergence_still_fails_the_run() -> Result<(), WorkflowError> {
    // A journal for `agent("once")`, replayed against a modified script that
    // first issues a mismatched call, catches the divergence, then issues the
    // originally journaled call. The run must still fail and never call the host.
    let base = format!("{META}\nlet r = agent(\"once\"); complete(r.output);");
    let first = Engine::new(EchoHost).run_script(&base, RunOptions::default())?;
    let serialized = first.journal.to_json()?;

    let attack = format!(
        r#"{META}
try {{ agent("wrong"); }} catch (err) {{ log("swallowed"); }}
let r = agent("once");
complete(r.output);
"#
    );
    let calls = Rc::new(Cell::new(0));
    let result = Engine::new(CountingHost {
        calls: Rc::clone(&calls),
    })
    .run_script(
        &attack,
        RunOptions {
            journal: Journal::from_json(&serialized)?,
            ..RunOptions::default()
        },
    );

    assert!(matches!(result, Err(WorkflowError::JournalDivergence(_))));
    assert_eq!(calls.get(), 0, "host must not be invoked on divergence");
    Ok(())
}

#[test]
fn partial_parallel_panel_resumes_committed_slots() -> Result<(), WorkflowError> {
    // A journal for an admitted panel whose first slot committed but whose
    // second slot is still pending. Replay must reuse the committed slot and
    // only launch the missing one.
    let script = format!(
        r#"{META}
let out = parallel([#{{ prompt: "a" }}, #{{ prompt: "b" }}]);
complete(#{{ first: out[0].output, second: out[1].output }});
"#
    );
    let committed = AgentResult {
        agent_id: "recorded-a".to_owned(),
        success: true,
        output: "recorded-a-output".to_owned(),
        cancelled: false,
        tokens_used: 7,
        duration_ms: 0,
    };
    let journal = Journal::from_entries(vec![JournalEntry::Parallel {
        invocation: 0,
        requests: vec![plain_request("a"), plain_request("b")],
        slots: vec![
            ParallelSlot::Completed {
                result: Some(committed),
            },
            ParallelSlot::Pending,
        ],
    }])?;

    let calls = Rc::new(Cell::new(0));
    let result = Engine::new(CountingHost {
        calls: Rc::clone(&calls),
    })
    .run_script(
        &script,
        RunOptions {
            journal,
            ..RunOptions::default()
        },
    )?;

    assert_eq!(calls.get(), 1, "only the missing slot should launch");
    assert_eq!(
        result.complete,
        Some(json!({"first": "recorded-a-output", "second": "b"}))
    );
    Ok(())
}

#[test]
fn checkpoint_survives_abort_and_resume_skips_completed_calls() -> Result<(), WorkflowError> {
    let dir = TempDir::new("abort-checkpoint")?;
    let checkpoint = dir.path().join("run.json");
    let script = format!(
        r#"{META}
let first = agent("one");
let second = agent("two");
complete(first.output + second.output);
"#
    );
    let aborted_calls = Rc::new(Cell::new(0));
    let aborted = Engine::new(SecondCallHost {
        calls: Rc::clone(&aborted_calls),
        outcome: SecondCallOutcome::InfrastructureFailure,
    })
    .run_script(
        &script,
        RunOptions {
            checkpoint: Some(checkpoint.clone()),
            ..RunOptions::default()
        },
    );
    assert!(matches!(aborted, Err(WorkflowError::Runtime(_))));
    assert_eq!(aborted_calls.get(), 2);

    let saved = Journal::from_json(&fs::read_to_string(&checkpoint)?)?;
    assert_eq!(saved.entries().len(), 1);
    let resumed_calls = Rc::new(Cell::new(0));
    let resumed = Engine::new(CountingHost {
        calls: Rc::clone(&resumed_calls),
    })
    .run_script(
        &script,
        RunOptions {
            journal: saved,
            checkpoint: Some(checkpoint.clone()),
            ..RunOptions::default()
        },
    )?;

    assert_eq!(resumed_calls.get(), 1, "the first agent must be replayed");
    assert_eq!(resumed.complete, Some(json!("onetwo")));
    assert_eq!(
        Journal::from_json(&fs::read_to_string(checkpoint)?)?
            .entries()
            .len(),
        2
    );
    Ok(())
}

#[test]
fn retry_failed_rewinds_only_failed_parallel_slots_and_dependents() -> Result<(), WorkflowError> {
    let failed = AgentResult {
        agent_id: "failed-b".into(),
        success: false,
        output: "failed".into(),
        cancelled: false,
        tokens_used: 1,
        duration_ms: 0,
    };
    let mut journal = Journal::from_entries(vec![
        JournalEntry::Agent {
            invocation: 0,
            request: plain_request("plan"),
            result: successful_result("plan-agent", "plan"),
        },
        JournalEntry::Parallel {
            invocation: 1,
            requests: vec![plain_request("a"), plain_request("b"), plain_request("c")],
            slots: vec![
                ParallelSlot::Completed {
                    result: Some(successful_result("agent-a", "a")),
                },
                ParallelSlot::Completed {
                    result: Some(failed),
                },
                ParallelSlot::Completed { result: None },
            ],
        },
        JournalEntry::Agent {
            invocation: 2,
            request: plain_request("stale-dependent"),
            result: successful_result("stale-agent", "stale"),
        },
    ])?;

    assert_eq!(journal.retry_failed(), 2);
    assert_eq!(journal.entries().len(), 2);
    let JournalEntry::Parallel { slots, .. } = &journal.entries()[1] else {
        panic!("second entry must remain a parallel panel");
    };
    assert!(matches!(
        slots[0],
        ParallelSlot::Completed {
            result: Some(ref result)
        } if result.success
    ));
    assert!(matches!(slots[1], ParallelSlot::Pending));
    assert!(matches!(slots[2], ParallelSlot::Pending));

    let calls = Rc::new(Cell::new(0));
    let script = format!(
        r#"{META}
agent("plan");
let panel = parallel([
    #{{ prompt: "a" }},
    #{{ prompt: "b" }},
    #{{ prompt: "c" }}
]);
let final = agent(panel[0].output + panel[1].output + panel[2].output);
complete(final.output);
"#
    );
    let resumed = Engine::new(CountingHost {
        calls: Rc::clone(&calls),
    })
    .run_script(
        &script,
        RunOptions {
            journal,
            ..RunOptions::default()
        },
    )?;
    assert_eq!(
        calls.get(),
        3,
        "two failed slots and one dependent agent rerun"
    );
    assert_eq!(resumed.complete, Some(json!("abc")));
    Ok(())
}

#[test]
fn parallel_result_count_mismatch_is_rejected() {
    let malformed = r#"{
        "entries": [{
            "call": "parallel",
            "invocation": 0,
            "requests": [
                {"prompt": "a", "options": {"label": null, "phase": null, "capability_mode": null, "output_schema": null, "agent_type": null, "model": null}},
                {"prompt": "b", "options": {"label": null, "phase": null, "capability_mode": null, "output_schema": null, "agent_type": null, "model": null}}
            ],
            "slots": [{"state": "pending"}]
        }]
    }"#;

    // `from_json` deserializes the raw shape and validates via `from_entries`,
    // so a cardinality violation surfaces as a divergence, not a JSON error.
    assert!(matches!(
        Journal::from_json(malformed),
        Err(WorkflowError::JournalDivergence(_))
    ));
    // The direct constructor reports the same invariant as a divergence.
    let mismatched = Journal::from_entries(vec![JournalEntry::Parallel {
        invocation: 0,
        requests: vec![plain_request("a"), plain_request("b")],
        slots: vec![ParallelSlot::Pending],
    }]);
    assert!(matches!(
        mismatched,
        Err(WorkflowError::JournalDivergence(_))
    ));
}

#[test]
fn directly_deserialized_invalid_journal_is_rejected() {
    // Non-contiguous invocation index must be rejected even via plain serde.
    let malformed = r#"{
        "entries": [{
            "call": "await_user",
            "invocation": 99,
            "kind": "input",
            "message": "need info"
        }]
    }"#;

    // A direct `serde_json::from_str::<Journal>` must still reject it (via the
    // validating `Deserialize` impl, whose error travels serde's channel).
    let via_serde: Result<Journal, _> = serde_json::from_str(malformed);
    assert!(via_serde.is_err(), "serde must run journal validation");
    // The public `from_json` API reports the precise divergence error type.
    assert!(matches!(
        Journal::from_json(malformed),
        Err(WorkflowError::JournalDivergence(_))
    ));
}

#[test]
fn nested_block_comments_in_meta_are_accepted() -> Result<(), WorkflowError> {
    let script = r#"let meta = #{
        /* outer /* nested */ still outer */
        name: "nested-comment",
        description: "Handles nested block comments"
    };
    complete(true);
    "#;
    let workflow = Engine::new(EchoHost).compile(script)?;
    assert_eq!(workflow.metadata().name, "nested-comment");
    Ok(())
}

#[test]
fn complete_cannot_be_caught() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
try {{ complete("done"); }} catch (err) {{ log("caught"); }}
log("after");
"#
    );
    let result = Engine::new(EchoHost).run_script(&script, RunOptions::default())?;

    assert_eq!(result.complete, Some(json!("done")));
    assert!(result.logs.is_empty());
    Ok(())
}

/// A non-echo host that grants every capability, used to exercise the
/// capability model without a real LLM backend.
#[derive(Clone)]
struct AllHost {
    calls: Rc<Cell<usize>>,
}

impl Host for AllHost {
    fn granted_capability(&self) -> Capability {
        Capability::All
    }

    fn run_agent(
        &self,
        request: &AgentRequest,
    ) -> Result<AgentResult, HostError> {
        self.calls.set(self.calls.get() + 1);
        Ok(AgentResult {
            agent_id: format!("all-{}", self.calls.get()),
            success: true,
            output: request.prompt.clone(),
            cancelled: false,
            tokens_used: 1,
            duration_ms: 0,
        })
    }
}

/// A self-cleaning temporary directory for filesystem tests.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Result<Self, WorkflowError> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ren-wf-{}-{tag}-{unique}", std::process::id()));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_workflow(
    dir: &std::path::Path,
    file_stem: &str,
    name: &str,
) -> Result<PathBuf, WorkflowError> {
    let path = dir.join(format!("{file_stem}.rhai"));
    let script = format!(
        "let meta = #{{ name: \"{name}\", description: \"{name} desc\", when_to_use: \"use {name}\" }};\ncomplete(\"{name}\");\n"
    );
    fs::write(&path, script)?;
    Ok(path)
}

#[test]
fn capability_string_parses_and_orders() {
    assert_eq!(
        "read-only".parse::<Capability>().ok(),
        Some(Capability::ReadOnly)
    );
    assert_eq!(
        "read-write".parse::<Capability>().ok(),
        Some(Capability::ReadWrite)
    );
    assert_eq!(
        "execute".parse::<Capability>().ok(),
        Some(Capability::Execute)
    );
    assert_eq!("all".parse::<Capability>().ok(), Some(Capability::All));
    assert!("nope".parse::<Capability>().is_err());
    assert!(Capability::Execute > Capability::ReadOnly);
}

#[test]
fn read_only_host_rejects_execute_agent() {
    let script = format!(
        r#"{META}
agent("do", #{{ capability_mode: "execute" }});
complete(true);
"#
    );
    let calls = Rc::new(Cell::new(0));
    let result = Engine::new(CountingHost {
        calls: Rc::clone(&calls),
    })
    .run_script(&script, RunOptions::default());

    assert!(
        matches!(
            result,
            Err(WorkflowError::CapabilityDenied {
                requested: Capability::Execute,
                granted: Capability::ReadOnly,
            })
        ),
        "expected typed capability rejection"
    );
    assert_eq!(calls.get(), 0, "host must not be invoked on rejection");
}

#[test]
fn invalid_capability_string_errors() {
    let script = format!(
        r#"{META}
agent("do", #{{ capability_mode: "sudo" }});
complete(true);
"#
    );
    let calls = Rc::new(Cell::new(0));
    let result = Engine::new(CountingHost {
        calls: Rc::clone(&calls),
    })
    .run_script(&script, RunOptions::default());

    assert!(
        matches!(
            result,
            Err(WorkflowError::InvalidCapabilityMode(mode)) if mode == "sudo"
        ),
        "expected typed invalid capability error"
    );
    assert_eq!(calls.get(), 0);
}

#[test]
fn capability_rejection_cannot_be_caught() {
    let script = format!(
        r#"{META}
try {{ agent("do", #{{ capability_mode: "execute" }}); }} catch (err) {{ log("swallowed"); }}
complete(true);
"#
    );
    let result = Engine::new(CountingHost {
        calls: Rc::new(Cell::new(0)),
    })
    .run_script(&script, RunOptions::default());
    assert!(matches!(
        result,
        Err(WorkflowError::CapabilityDenied { .. })
    ));
}

#[test]
fn permissive_host_allows_execute_agent() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
let r = agent("do", #{{ capability_mode: "execute" }});
complete(r.output);
"#
    );
    let calls = Rc::new(Cell::new(0));
    let result = crate::run_with_host(
        AllHost {
            calls: Rc::clone(&calls),
        },
        &script,
        RunOptions::default(),
    )?;

    assert_eq!(result.complete, Some(json!("do")));
    assert_eq!(calls.get(), 1);
    Ok(())
}

#[test]
fn replay_rejects_calls_recorded_under_stronger_host() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
let r = agent("do", #{{ capability_mode: "execute" }});
complete(r.output);
"#
    );
    let calls = Rc::new(Cell::new(0));
    let recorded = Engine::new(AllHost {
        calls: Rc::clone(&calls),
    })
    .run_script(&script, RunOptions::default())?;
    assert_eq!(calls.get(), 1);

    let replay = Engine::new(CountingHost {
        calls: Rc::new(Cell::new(0)),
    })
    .run_script(
        &script,
        RunOptions {
            journal: recorded.journal,
            ..RunOptions::default()
        },
    );
    assert!(matches!(
        replay,
        Err(WorkflowError::CapabilityDenied {
            requested: Capability::Execute,
            granted: Capability::ReadOnly,
        })
    ));
    Ok(())
}

#[test]
fn meta_extracts_when_to_use_and_args_schema() -> Result<(), WorkflowError> {
    let script = r#"let meta = #{
        name: "with-schema",
        description: "Has a schema",
        when_to_use: "When you need a schema",
        args_schema: #{
            type: "object",
            required: ["topic"],
            properties: #{ topic: #{ type: "string" } }
        }
    };
    complete(true);
    "#;
    let metadata = Engine::new(EchoHost).compile(script)?.metadata().clone();
    assert_eq!(
        metadata.when_to_use.as_deref(),
        Some("When you need a schema")
    );
    let schema = metadata
        .args_schema
        .ok_or_else(|| WorkflowError::InvalidConfig("args_schema missing".into()))?;
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["required"], json!(["topic"]));
    Ok(())
}

#[test]
fn discovery_prefers_project_over_user() -> Result<(), WorkflowError> {
    let project = TempDir::new("project")?;
    let user = TempDir::new("user")?;
    let project_path = write_workflow(project.path(), "shared", "shared")?;
    // This shadowed user entry also has a stem mismatch; neither condition
    // should produce a warning because the project entry wins cleanly.
    write_workflow(user.path(), "shadowed-file", "shared")?;
    write_workflow(user.path(), "user-only", "user-only")?;

    let discovery = registry::discover_in(project.path(), Some(user.path()));
    let shared = discovery
        .workflows
        .iter()
        .find(|workflow| workflow.name == "shared")
        .ok_or_else(|| WorkflowError::InvalidConfig("shared missing".into()))?;
    assert_eq!(shared.source, WorkflowSource::Project);
    assert_eq!(shared.path, project_path);

    let user_only = discovery
        .workflows
        .iter()
        .find(|workflow| workflow.name == "user-only")
        .ok_or_else(|| WorkflowError::InvalidConfig("user-only missing".into()))?;
    assert_eq!(user_only.source, WorkflowSource::User);
    assert!(
        discovery.warnings.is_empty(),
        "shadowed entries should not emit warnings"
    );
    Ok(())
}

#[test]
fn discovery_warns_on_name_mismatch_and_bad_files() -> Result<(), WorkflowError> {
    let project = TempDir::new("warn")?;
    write_workflow(project.path(), "mismatch", "actual-name")?;
    fs::write(project.path().join("broken.rhai"), "not valid meta")?;

    let discovery = registry::discover_in(project.path(), None);
    assert!(discovery.workflows.iter().any(|w| w.name == "actual-name"));
    assert_eq!(discovery.warnings.len(), 2, "mismatch + broken warnings");
    Ok(())
}

#[test]
fn discovery_warns_on_duplicate_name_within_directory() -> Result<(), WorkflowError> {
    let project = TempDir::new("duplicate")?;
    let retained = write_workflow(project.path(), "duplicate", "duplicate")?;
    let dropped = write_workflow(project.path(), "z-duplicate", "duplicate")?;

    let discovery = registry::discover_in(project.path(), None);
    assert_eq!(
        discovery
            .workflows
            .iter()
            .filter(|workflow| workflow.name == "duplicate")
            .count(),
        1
    );
    assert_eq!(
        discovery
            .workflows
            .iter()
            .find(|workflow| workflow.name == "duplicate")
            .map(|workflow| &workflow.path),
        Some(&retained)
    );
    assert!(discovery.warnings.iter().any(|warning| {
        warning.path == dropped && warning.reason.contains("duplicate meta.name `duplicate`")
    }));
    Ok(())
}

#[test]
fn resolve_by_name_and_load_by_path() -> Result<(), WorkflowError> {
    let project = TempDir::new("resolve")?;
    let path = write_workflow(project.path(), "resolvable", "resolvable")?;
    let discovery = registry::discover_in(project.path(), None);

    let resolved = registry::resolve_in("resolvable", &discovery)?;
    assert_eq!(resolved.path, path);
    assert!(registry::resolve_in("missing", &discovery).is_err());

    // Run-by-path loading accepts an existing `.rhai` file directly.
    let by_path = crate::load_target(&path.to_string_lossy())?;
    assert_eq!(by_path, fs::read_to_string(path)?);
    Ok(())
}

#[test]
fn schema_export_shape() {
    let metadata = crate::WorkflowMeta {
        name: "deep-research".into(),
        description: "desc".into(),
        when_to_use: None,
        args_schema: Some(json!({ "type": "object", "required": ["topic"] })),
        phases: Vec::new(),
    };
    let descriptor = schema::tool_descriptor(&metadata);
    assert_eq!(descriptor["name"], json!("deep-research"));
    assert_eq!(descriptor["description"], json!("desc"));
    assert_eq!(descriptor["inputSchema"]["required"], json!(["topic"]));

    let no_schema = crate::WorkflowMeta {
        name: "bare".into(),
        description: "d".into(),
        when_to_use: None,
        args_schema: None,
        phases: Vec::new(),
    };
    let descriptor = schema::tool_descriptor(&no_schema);
    assert_eq!(descriptor["inputSchema"], json!({ "type": "object" }));
}

#[test]
fn args_validation_against_schema() {
    let schema = json!({
        "type": "object",
        "required": ["topic"],
        "properties": {
            "topic": { "type": "string", "minLength": 1 },
            "rounds": { "type": "integer", "minimum": 2, "maximum": 6 }
        }
    });

    assert!(schema::validate_args(&schema, Some(&json!({ "topic": "x" }))).is_ok());
    assert!(schema::validate_args(&schema, Some(&json!({ "topic": "x", "rounds": 3 }))).is_ok());
    assert!(schema::validate_args(&schema, Some(&json!({ "topic": "x", "rounds": 3.0 }))).is_ok());
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!({ "topic": "" }))),
        Err(WorkflowError::InvalidConfig(_))
    ));
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!({ "topic": "x", "rounds": 1 }))),
        Err(WorkflowError::InvalidConfig(_))
    ));
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!({ "topic": "x", "rounds": 7 }))),
        Err(WorkflowError::InvalidConfig(_))
    ));
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!({ "topic": "x", "rounds": 3.5 }))),
        Err(WorkflowError::InvalidConfig(_))
    ));
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!({ "rounds": 3 }))),
        Err(WorkflowError::InvalidConfig(_))
    ));
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!({ "topic": 5 }))),
        Err(WorkflowError::InvalidConfig(_))
    ));
    assert!(matches!(
        schema::validate_args(&schema, Some(&json!([1, 2]))),
        Err(WorkflowError::InvalidConfig(_))
    ));
}

#[test]
fn args_validation_rejects_additional_properties_when_disabled() {
    let strict = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "topic": { "type": "string" }
        }
    });
    let unexpected = json!({ "topic": "x", "unexpected": true });

    let error = schema::validate_args(&strict, Some(&unexpected))
        .expect_err("strict object schema must reject undeclared fields");
    assert!(matches!(
        error,
        WorkflowError::InvalidConfig(ref message)
            if message == "args do not match schema: unexpected field `unexpected`"
    ));

    let permissive = json!({
        "type": "object",
        "properties": {
            "topic": { "type": "string" }
        }
    });
    assert!(schema::validate_args(&permissive, Some(&unexpected)).is_ok());
}

#[test]
fn cli_run_journal_save_and_resume_round_trip() -> Result<(), WorkflowError> {
    let dir = TempDir::new("journal")?;
    let script_path = dir.path().join("echo.rhai");
    fs::write(
        &script_path,
        format!("{META}\nlet r = agent(\"hello\"); complete(r.output);\n"),
    )?;
    let out = dir.path().join("journal.json");

    crate::run(crate::Config::for_run(crate::RunArgs {
        target: script_path.to_string_lossy().into_owned(),
        args: None,
        agent_budget: 8,
        journal: Some(out.clone()),
        resume: None,
        retry_failed: false,
    }))?;

    let saved = fs::read_to_string(&out)?;
    let journal = Journal::from_json(&saved)?;
    assert_eq!(journal.entries().len(), 1);

    // Resuming from the saved journal replays deterministically and re-emits an
    // identical journal.
    let out2 = dir.path().join("journal2.json");
    crate::run(crate::Config::for_run(crate::RunArgs {
        target: script_path.to_string_lossy().into_owned(),
        args: None,
        agent_budget: 8,
        journal: Some(out2.clone()),
        resume: Some(out.clone()),
        retry_failed: false,
    }))?;
    assert_eq!(fs::read_to_string(&out)?, fs::read_to_string(&out2)?);
    Ok(())
}

#[test]
fn cli_retry_failed_updates_the_resume_checkpoint_in_place() -> Result<(), WorkflowError> {
    let dir = TempDir::new("retry-failed")?;
    let script_path = dir.path().join("retry.rhai");
    fs::write(
        &script_path,
        format!("{META}\nlet r = agent(\"retry me\"); complete(r.output);\n"),
    )?;
    let checkpoint = dir.path().join("run.json");
    let failed = AgentResult {
        agent_id: "failed-agent".into(),
        success: false,
        output: "temporary model failure".into(),
        cancelled: false,
        tokens_used: 1,
        duration_ms: 0,
    };
    Journal::from_entries(vec![JournalEntry::Agent {
        invocation: 0,
        request: plain_request("retry me"),
        result: failed,
    }])?
    .write_atomic(&checkpoint)?;

    crate::run(crate::Config::for_run(crate::RunArgs {
        target: script_path.to_string_lossy().into_owned(),
        args: None,
        agent_budget: 8,
        journal: None,
        resume: Some(checkpoint.clone()),
        retry_failed: true,
    }))?;

    let saved = Journal::from_json(&fs::read_to_string(checkpoint)?)?;
    let [JournalEntry::Agent { result, .. }] = saved.entries() else {
        panic!("retry must leave one completed agent entry");
    };
    assert!(result.success);
    assert_eq!(result.output, "retry me");
    Ok(())
}

#[test]
fn cli_run_rejects_args_violating_schema() -> Result<(), WorkflowError> {
    let dir = TempDir::new("argsval")?;
    let script_path = dir.path().join("schema.rhai");
    fs::write(
        &script_path,
        "let meta = #{ name: \"s\", description: \"d\", args_schema: #{ type: \"object\", additionalProperties: false, required: [\"topic\"], properties: #{ topic: #{ type: \"string\" } } } };\ncomplete(true);\n",
    )?;

    for (args, expected) in [
        ("{\"nope\": 1}", "missing required field `topic`"),
        ("{\"topic\": \"x\", \"nope\": 1}", "unexpected field `nope`"),
    ] {
        let result = crate::run(crate::Config::for_run(crate::RunArgs {
            target: script_path.to_string_lossy().into_owned(),
            args: Some(args.into()),
            agent_budget: 8,
            journal: None,
            resume: None,
            retry_failed: false,
        }));
        assert!(
            matches!(result, Err(WorkflowError::InvalidConfig(ref message)) if message.contains(expected)),
            "expected args error containing `{expected}`, got {result:?}"
        );
    }
    Ok(())
}

#[test]
fn registry_paths_use_ren_namespace_for_project_and_user() -> Result<(), WorkflowError> {
    let project = TempDir::new("ren-project")?;
    fs::create_dir(project.path().join(".git"))?;
    let project_workflows = project.path().join(".ren/workflows");
    fs::create_dir_all(&project_workflows)?;
    write_workflow(&project_workflows, "project-skill", "project-skill")?;

    let home = TempDir::new("ren-home")?;
    let user_workflows = registry::user_workflow_dir_in(home.path());
    fs::create_dir_all(&user_workflows)?;
    write_workflow(&user_workflows, "user-skill", "user-skill")?;

    assert_eq!(
        registry::project_workflow_dir_in(project.path()),
        project_workflows
    );
    assert_eq!(user_workflows, home.path().join(".ren/workflows"));
    let discovery = registry::discover_in(&project_workflows, Some(&user_workflows));
    assert_eq!(
        discovery.workflows.len(),
        2 + crate::bundled::WORKFLOWS.len()
    );
    assert!(
        discovery
            .workflows
            .iter()
            .any(|workflow| workflow.name == "project-skill")
    );
    assert!(
        discovery
            .workflows
            .iter()
            .any(|workflow| workflow.name == "user-skill")
    );
    assert!(!project.path().join(".grok/workflows").exists());
    assert!(!home.path().join(".grok/workflows").exists());
    Ok(())
}

#[test]
fn bridge_paths_and_contents_cover_every_agent_and_scope() -> Result<(), WorkflowError> {
    let base = TempDir::new("bridges")?;
    let cases = [
        (crate::bridge::Agent::Claude, ".claude/commands/ren.md"),
        (crate::bridge::Agent::Cursor, ".cursor/commands/ren.md"),
        (crate::bridge::Agent::Codex, ".codex/prompts/ren.md"),
        (crate::bridge::Agent::Grok, ".grok/commands/ren.md"),
    ];
    for (agent, relative) in cases {
        for scope in [
            crate::bridge::BridgeScope::Global,
            crate::bridge::BridgeScope::Project,
        ] {
            let definition = crate::bridge::bridge_definition(base.path(), agent, scope);
            assert_eq!(definition.path, base.path().join(relative));
            assert_eq!(definition.contents, crate::bridge::DISPATCHER_CONTENT);
            assert!(definition.contents.contains("ren workflow run $ARGUMENTS"));
            crate::bridge::install_bridge(&definition, false)?;
            assert_eq!(fs::read_to_string(&definition.path)?, definition.contents);
            assert!(matches!(
                crate::bridge::install_bridge(&definition, false),
                Err(WorkflowError::BridgeExists(_))
            ));
            crate::bridge::install_bridge(&definition, true)?;
            assert!(crate::bridge::uninstall_bridge(&definition)?);
            assert!(!crate::bridge::uninstall_bridge(&definition)?);
        }
    }
    Ok(())
}

#[test]
fn init_installs_skill_for_every_agent_and_scope() -> Result<(), WorkflowError> {
    let base = TempDir::new("skills")?;
    let cases = [
        (crate::bridge::Agent::Claude, ".claude/skills/ren-workflow"),
        (crate::bridge::Agent::Cursor, ".cursor/skills/ren-workflow"),
        (crate::bridge::Agent::Codex, ".codex/skills/ren-workflow"),
        (crate::bridge::Agent::Grok, ".grok/skills/ren-workflow"),
    ];
    for (agent, relative_dir) in cases {
        let definition = crate::init::skill_definition(base.path(), agent);
        assert_eq!(definition.dir, base.path().join(relative_dir));
        assert_eq!(definition.files.len(), crate::init::SKILL_FILES.len());

        crate::init::install_skill(&definition, false)?;
        let skill_md = definition.dir.join("SKILL.md");
        assert_eq!(fs::read_to_string(&skill_md)?, crate::init::SKILL_MD);
        // The thin bootstrap ships only SKILL.md; rich guidance lives in the
        // binary and is fetched via `workflow protocol`, not installed here.
        assert!(!definition.dir.join("references").exists());

        // A byte-identical install is idempotent without --force.
        crate::init::install_skill(&definition, false)?;
        assert_eq!(fs::read_to_string(&skill_md)?, crate::init::SKILL_MD);
        fs::write(&skill_md, "user-owned contents")?;
        assert!(matches!(
            crate::init::install_skill(&definition, false),
            Err(WorkflowError::SkillExists(path)) if path == skill_md
        ));
        assert_eq!(fs::read_to_string(&skill_md)?, "user-owned contents");
        // With --force it overwrites cleanly.
        crate::init::install_skill(&definition, true)?;
        assert_eq!(fs::read_to_string(&skill_md)?, crate::init::SKILL_MD);
    }
    Ok(())
}

#[test]
fn init_preflights_every_file_before_writing() -> Result<(), WorkflowError> {
    const FILES: &[crate::init::SkillFile] = &[
        crate::init::SkillFile {
            relative: "SKILL.md",
            contents: "skill contents",
        },
        crate::init::SkillFile {
            relative: "agents/openai.yaml",
            contents: "generated metadata",
        },
    ];
    let base = TempDir::new("skill-preflight")?;
    let definition = crate::init::skill_definition_for(
        base.path(),
        crate::bridge::Agent::Codex,
        crate::init::EmbeddedSkill {
            name: "test-skill",
            files: FILES,
        },
    );
    let metadata = definition.dir.join("agents/openai.yaml");
    fs::create_dir_all(
        metadata.parent().ok_or_else(|| {
            WorkflowError::InvalidConfig("metadata path must have a parent".into())
        })?,
    )?;
    fs::write(&metadata, "user-owned metadata")?;

    assert!(matches!(
        crate::init::install_skill(&definition, false),
        Err(WorkflowError::SkillExists(path)) if path == metadata
    ));
    assert!(!definition.dir.join("SKILL.md").exists());
    assert_eq!(fs::read_to_string(metadata)?, "user-owned metadata");
    Ok(())
}

#[test]
fn init_rolls_back_files_after_an_apply_failure() -> Result<(), WorkflowError> {
    let base = TempDir::new("skill-rollback")?;
    let output_dir = base.path().join("output");
    let first_file = output_dir.join("first");
    let definition = crate::init::SkillDefinition {
        dir: output_dir.clone(),
        // Both paths are absent during preflight. Writing the first target
        // creates `output/` as a directory, making the second target fail.
        files: vec![
            (first_file.clone(), "first contents"),
            (output_dir, "cannot replace a directory"),
        ],
    };

    assert!(matches!(
        crate::init::install_skill(&definition, false),
        Err(WorkflowError::Io(_))
    ));
    assert!(
        !first_file.exists(),
        "the earlier file must be removed when the batch fails"
    );
    Ok(())
}

#[test]
fn supported_agents_are_unique_and_embedded_skill_is_valid() -> Result<(), WorkflowError> {
    let agents = crate::init::supported_agents();
    for (index, agent) in agents.iter().enumerate() {
        assert!(
            !agents[..index].contains(agent),
            "duplicate agent {agent:?}"
        );
    }
    assert!(
        crate::init::SKILL_MD.starts_with("---\nname: ren-workflow\n"),
        "embedded SKILL.md must begin with Agent Skills frontmatter"
    );
    let frontmatter = crate::init::SKILL_MD
        .strip_prefix("---\n")
        .and_then(|skill| skill.split_once("\n---\n"))
        .map(|(frontmatter, _)| frontmatter)
        .ok_or_else(|| WorkflowError::InvalidConfig("skill frontmatter is not closed".into()))?;
    let keys = frontmatter
        .lines()
        .filter(|line| !line.chars().next().is_some_and(char::is_whitespace))
        .filter_map(|line| line.split_once(':').map(|(key, _)| key))
        .collect::<Vec<_>>();
    assert_eq!(keys, ["name", "description"]);
    // The thin bootstrap defers to the binary rather than duplicating guidance.
    assert!(crate::init::SKILL_MD.contains("agent_protocol"));
    assert!(!crate::guide::PROTOCOL_MD.is_empty());
    assert!(!crate::guide::AUTHORING_MD.is_empty());
    assert!(
        crate::guide::PROTOCOL_MD.contains("Enforce structured outputs"),
        "the executor protocol must fail closed on schema-invalid real output"
    );
    assert!(
        crate::guide::PROTOCOL_MD.contains("Never silently repair"),
        "the executor protocol must preserve the real output trust boundary"
    );
    Ok(())
}

#[test]
fn run_result_carries_version_matched_agent_protocol() -> Result<(), WorkflowError> {
    let script = format!(
        r#"{META}
phase("Work");
let result = agent("hello");
complete(#{{ answer: result.output }});
"#
    );
    let result = Engine::new(EchoHost).run_script(&script, RunOptions::default())?;
    assert_eq!(result.agent_protocol, crate::guide::PROTOCOL_MD);
    assert!(!result.agent_protocol.is_empty());
    Ok(())
}

#[test]
fn every_bundled_workflow_has_matching_valid_metadata_and_compiles() -> Result<(), WorkflowError> {
    let engine = Engine::new(EchoHost);
    assert_eq!(crate::bundled::WORKFLOWS.len(), 2);
    for bundled in crate::bundled::WORKFLOWS {
        let metadata = crate::meta::extract(bundled.source)?;
        let stem = std::path::Path::new(bundled.file_name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| WorkflowError::InvalidConfig("bundled file stem missing".into()))?;
        assert_eq!(bundled.name, metadata.name);
        assert_eq!(stem, metadata.name);
        engine.compile(bundled.source)?;
    }
    Ok(())
}

#[test]
fn bundled_implement_produces_bounded_review_and_fix_plan() -> Result<(), WorkflowError> {
    let workflow = crate::bundled::find("implement")
        .ok_or_else(|| WorkflowError::InvalidConfig("implement missing".into()))?;
    let result = Engine::new(EchoHost).run_script(
        workflow.source,
        RunOptions {
            args: Some(json!({
                "task": "Add a deterministic parser",
                "effort": 4,
                "review_rounds": 2
            })),
            agent_budget: 13,
            ..RunOptions::default()
        },
    )?;

    assert_eq!(result.phases, ["Implement", "Review and fix", "Report"]);
    assert_eq!(
        result
            .complete
            .as_ref()
            .map(|complete| &complete["reviewers_per_round"]),
        Some(&json!(5))
    );
    assert_eq!(
        result
            .complete
            .as_ref()
            .map(|complete| &complete["review_rounds"]),
        Some(&json!(2))
    );
    assert_eq!(
        result
            .complete
            .as_ref()
            .map(|complete| &complete["budget"]["spent"]),
        Some(&json!(13))
    );

    let requests = result
        .journal
        .entries()
        .iter()
        .flat_map(|entry| match entry {
            JournalEntry::Agent { request, .. } => vec![request],
            JournalEntry::Parallel { requests, .. } => requests.iter().collect(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 13);
    assert_eq!(
        requests[0].options.capability_mode.as_deref(),
        Some("execute")
    );
    assert_eq!(
        requests[1].options.capability_mode.as_deref(),
        Some("read-only")
    );
    assert_eq!(
        requests[6].options.capability_mode.as_deref(),
        Some("execute")
    );
    assert!(
        requests
            .iter()
            .all(|request| request.options.output_schema.is_some())
    );
    assert!(requests[6].prompt.contains("<review-packet-json>"));
    assert!(
        requests
            .last()
            .is_some_and(|request| request.prompt.contains("<final-review-packet-json>"))
    );
    Ok(())
}

#[test]
fn bundled_implement_rejects_invalid_values_even_without_cli_schema_validation() {
    let workflow = crate::bundled::find("implement").expect("implement must be bundled");
    for (args, expected) in [
        (json!({"task": ""}), "task must not be empty"),
        (
            json!({"task": "Do work", "effort": 0}),
            "effort must be between 1 and 5",
        ),
        (
            json!({"task": "Do work", "effort": 6}),
            "effort must be between 1 and 5",
        ),
        (
            json!({"task": "Do work", "review_rounds": 0}),
            "review_rounds must be between 1 and 8",
        ),
        (
            json!({"task": "Do work", "review_rounds": 9}),
            "review_rounds must be between 1 and 8",
        ),
    ] {
        let result = Engine::new(EchoHost).run_script(
            workflow.source,
            RunOptions {
                args: Some(args),
                ..RunOptions::default()
            },
        );
        assert!(
            matches!(result, Err(WorkflowError::Runtime(ref message)) if message.contains(expected)),
            "expected runtime error containing `{expected}`, got {result:?}"
        );
    }
}

#[test]
fn bundled_deep_research_produces_a_complete_bounded_plan() -> Result<(), WorkflowError> {
    let workflow = crate::bundled::find("deep-research")
        .ok_or_else(|| WorkflowError::InvalidConfig("deep-research missing".into()))?;
    let result = Engine::new(EchoHost).run_script(
        workflow.source,
        RunOptions {
            args: Some(json!({"query": "How does ren work?", "breadth": 2})),
            agent_budget: 6,
            ..RunOptions::default()
        },
    )?;

    assert_eq!(result.phases, ["Plan", "Research", "Verify", "Report"]);
    assert_eq!(result.logs.len(), 3);
    let complete = result.complete.as_ref().expect("complete result");
    assert_eq!(complete["scratch_key"].as_str(), Some("report.md"));
    assert_eq!(
        complete["artifact_persistence"].as_str(),
        Some("executor-required")
    );
    assert_eq!(complete["breadth"], json!(2));
    assert_eq!(complete["verifier_count"], json!(2));
    assert_eq!(complete["claims_per_question_max"], json!(10));
    assert_eq!(complete["budget"]["spent"], json!(6));

    let requests = result
        .journal
        .entries()
        .iter()
        .flat_map(|entry| match entry {
            JournalEntry::Agent { request, .. } => vec![request],
            JournalEntry::Parallel { requests, .. } => requests.iter().collect(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 6);
    assert!(
        requests
            .iter()
            .all(|request| request.options.output_schema.is_some())
    );
    let planner_schema = requests[0]
        .options
        .output_schema
        .as_ref()
        .expect("planner schema");
    assert_eq!(planner_schema["additionalProperties"], json!(false));
    assert_eq!(
        planner_schema["properties"]["questions"]["items"]["properties"]["question"]["minLength"],
        json!(1)
    );
    assert_eq!(
        planner_schema["properties"]["questions"]["items"]["required"],
        json!(["id", "question", "evidence_target", "angle", "claim_target"])
    );
    assert!(
        requests[0]
            .prompt
            .contains("lightweight preliminary research")
    );
    let research_schema = requests[1]
        .options
        .output_schema
        .as_ref()
        .expect("research schema");
    assert_eq!(
        research_schema["properties"]["claims"]["items"]["required"],
        json!(["claim_id", "claim", "evidence", "source_title", "locator"])
    );
    assert_eq!(
        research_schema["properties"]["claims"]["maxItems"],
        json!(10)
    );
    let verification_schema = requests[3]
        .options
        .output_schema
        .as_ref()
        .expect("verification schema");
    assert_eq!(
        verification_schema["properties"]["retained"]["items"]["required"][0],
        json!("claim_id")
    );
    assert!(
        requests[3]
            .prompt
            .contains("preserve its claim_id unchanged")
    );
    assert!(
        requests[5]
            .prompt
            .contains("verifier assigned to its research question")
    );
    assert!(
        requests
            .last()
            .is_some_and(|request| request.prompt.contains("<research-packet-json>"))
    );
    Ok(())
}

#[test]
fn bundled_deep_research_rejects_invalid_values_even_without_cli_schema_validation() {
    let workflow = crate::bundled::find("deep-research").expect("deep-research must be bundled");
    for (args, expected) in [
        (
            json!({"query": "", "breadth": 2}),
            "query must not be empty",
        ),
        (
            json!({"query": "How does ren work?", "breadth": 1}),
            "breadth must be at least 2",
        ),
    ] {
        let result = Engine::new(EchoHost).run_script(
            workflow.source,
            RunOptions {
                args: Some(args),
                ..RunOptions::default()
            },
        );
        assert!(
            matches!(result, Err(WorkflowError::Runtime(ref message)) if message.contains(expected)),
            "expected runtime error containing `{expected}`, got {result:?}"
        );
    }
}

#[test]
fn bundled_deep_research_scales_research_and_verification_with_breadth() -> Result<(), WorkflowError>
{
    let workflow = crate::bundled::find("deep-research")
        .ok_or_else(|| WorkflowError::InvalidConfig("deep-research missing".into()))?;
    let result = Engine::new(EchoHost).run_script(
        workflow.source,
        RunOptions {
            args: Some(json!({"query": "Map a large research domain", "breadth": 7})),
            agent_budget: 16,
            ..RunOptions::default()
        },
    )?;

    let requests = result
        .journal
        .entries()
        .iter()
        .flat_map(|entry| match entry {
            JournalEntry::Agent { request, .. } => vec![request],
            JournalEntry::Parallel { requests, .. } => requests.iter().collect(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 16);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .options
                .label
                .as_deref()
                .is_some_and(|label| { label.starts_with("researcher-") }))
            .count(),
        7
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request
                .options
                .label
                .as_deref()
                .is_some_and(|label| { label.starts_with("verifier-q") }))
            .count(),
        7
    );
    assert_eq!(
        result
            .complete
            .as_ref()
            .map(|complete| &complete["claims_per_question_max"]),
        Some(&json!(6))
    );
    Ok(())
}

#[test]
fn bundled_deep_research_rejects_breadth_that_exceeds_agent_budget() {
    let workflow = crate::bundled::find("deep-research").expect("deep-research must be bundled");
    let result = Engine::new(EchoHost).run_script(
        workflow.source,
        RunOptions {
            args: Some(json!({"query": "How does ren work?", "breadth": 3})),
            agent_budget: 7,
            ..RunOptions::default()
        },
    );
    assert!(
        matches!(result, Err(WorkflowError::Runtime(ref message))
            if message.contains("breadth does not fit the agent budget")),
        "expected an agent-budget error, got {result:?}"
    );
}

#[test]
fn bundled_deep_research_stops_on_unusable_parallel_results() {
    let workflow = crate::bundled::find("deep-research").expect("deep-research must be bundled");
    for (outcome, expected) in [
        (SecondCallOutcome::AgentFailure, "researcher-0 failed"),
        (SecondCallOutcome::Cancelled, "researcher-0 was cancelled"),
        (
            SecondCallOutcome::InfrastructureFailure,
            "researcher-0 is unavailable",
        ),
    ] {
        let calls = Rc::new(Cell::new(0));
        let result = Engine::new(SecondCallHost {
            calls: Rc::clone(&calls),
            outcome,
        })
        .run_script(
            workflow.source,
            RunOptions {
                args: Some(json!({"query": "How does ren work?", "breadth": 2})),
                agent_budget: 6,
                ..RunOptions::default()
            },
        );
        assert!(
            matches!(result, Err(WorkflowError::Runtime(ref message)) if message.contains(expected)),
            "expected runtime error containing `{expected}`, got {result:?}"
        );
        assert_eq!(
            calls.get(),
            3,
            "the admitted research panel completes first"
        );
    }
}

#[test]
fn bundled_deep_research_resumes_after_failed_researcher() -> Result<(), WorkflowError> {
    let workflow = crate::bundled::find("deep-research")
        .ok_or_else(|| WorkflowError::InvalidConfig("deep-research missing".into()))?;
    let dir = TempDir::new("deep-research-resume")?;
    let checkpoint = dir.path().join("run.json");
    let initial_calls = Rc::new(Cell::new(0));
    let initial = Engine::new(SecondCallHost {
        calls: Rc::clone(&initial_calls),
        outcome: SecondCallOutcome::AgentFailure,
    })
    .run_script(
        workflow.source,
        RunOptions {
            args: Some(json!({"query": "How does ren resume?", "breadth": 2})),
            agent_budget: 6,
            checkpoint: Some(checkpoint.clone()),
            ..RunOptions::default()
        },
    );
    assert!(matches!(
        initial,
        Err(WorkflowError::Runtime(ref message)) if message.contains("researcher-0 failed")
    ));
    assert_eq!(initial_calls.get(), 3);

    let mut journal = Journal::from_json(&fs::read_to_string(&checkpoint)?)?;
    assert_eq!(journal.retry_failed(), 1);
    journal.write_atomic(&checkpoint)?;

    let resumed_calls = Rc::new(Cell::new(0));
    let resumed = Engine::new(CountingHost {
        calls: Rc::clone(&resumed_calls),
    })
    .run_script(
        workflow.source,
        RunOptions {
            args: Some(json!({"query": "How does ren resume?", "breadth": 2})),
            journal,
            agent_budget: 6,
            checkpoint: Some(checkpoint),
        },
    )?;

    assert_eq!(
        resumed_calls.get(),
        4,
        "one researcher retries before verification and reporting"
    );
    assert_eq!(
        resumed
            .complete
            .as_ref()
            .and_then(|complete| complete["scratch_key"].as_str()),
        Some("report.md")
    );
    Ok(())
}

#[test]
fn discovery_uses_project_then_user_then_bundled_precedence() -> Result<(), WorkflowError> {
    let project = TempDir::new("three-tier-project")?;
    let user = TempDir::new("three-tier-user")?;

    let bundled_only = registry::discover_in(project.path(), Some(user.path()));
    let bundled = registry::resolve_in("deep-research", &bundled_only)?;
    assert_eq!(bundled.source, WorkflowSource::Bundled);
    assert_eq!(crate::source_label(bundled.source), "bundled");
    assert_eq!(
        registry::load_source(&bundled)?,
        crate::bundled::find("deep-research")
            .ok_or_else(|| WorkflowError::InvalidConfig("deep-research missing".into()))?
            .source
    );

    let user_path = write_workflow(user.path(), "deep-research", "deep-research")?;
    let user_wins = registry::discover_in(project.path(), Some(user.path()));
    let resolved_user = registry::resolve_in("deep-research", &user_wins)?;
    assert_eq!(resolved_user.source, WorkflowSource::User);
    assert_eq!(resolved_user.path, user_path);

    let project_path = write_workflow(project.path(), "deep-research", "deep-research")?;
    let project_wins = registry::discover_in(project.path(), Some(user.path()));
    let resolved_project = registry::resolve_in("deep-research", &project_wins)?;
    assert_eq!(resolved_project.source, WorkflowSource::Project);
    assert_eq!(resolved_project.path, project_path);
    assert!(project_wins.warnings.is_empty());
    Ok(())
}

#[test]
fn create_targets_project_by_default_and_user_when_selected() -> Result<(), WorkflowError> {
    let repository = TempDir::new("create-target-project")?;
    fs::create_dir(repository.path().join(".git"))?;
    let nested = repository.path().join("nested");
    fs::create_dir(&nested)?;
    let home = TempDir::new("create-target-home")?;

    assert_eq!(
        crate::create::store_dir(
            &nested,
            Some(home.path()),
            crate::create::CreateTarget::Project,
        )?,
        repository.path().join(".ren/workflows")
    );
    assert_eq!(
        crate::create::store_dir(
            &nested,
            Some(home.path()),
            crate::create::CreateTarget::User,
        )?,
        home.path().join(".ren/workflows")
    );
    assert!(matches!(
        crate::create::store_dir(&nested, None, crate::create::CreateTarget::User),
        Err(WorkflowError::HomeUnavailable)
    ));
    Ok(())
}

#[test]
fn generated_scaffold_is_valid_and_overwrite_requires_force() -> Result<(), WorkflowError> {
    let store = TempDir::new("create-scaffold")?;
    let plan = crate::create::create_in(store.path(), "demo-flow", None, false)?;
    assert_eq!(plan.path, store.path().join("demo-flow.rhai"));
    assert_eq!(fs::read_to_string(&plan.path)?, plan.contents);
    assert_eq!(crate::meta::extract(&plan.contents)?.name, "demo-flow");
    Engine::new(EchoHost).compile(&plan.contents)?;
    crate::run(crate::Config::for_run(crate::RunArgs {
        target: plan.path.to_string_lossy().into_owned(),
        args: None,
        agent_budget: 8,
        journal: None,
        resume: None,
        retry_failed: false,
    }))?;

    let original = fs::read_to_string(&plan.path)?;
    assert!(matches!(
        crate::create::create_in(store.path(), "demo-flow", None, false),
        Err(WorkflowError::WorkflowExists(path)) if path == plan.path
    ));
    assert_eq!(fs::read_to_string(&plan.path)?, original);
    fs::write(&plan.path, "different")?;
    crate::create::create_in(store.path(), "demo-flow", None, true)?;
    assert_eq!(
        crate::meta::extract(&fs::read_to_string(plan.path)?)?.name,
        "demo-flow"
    );
    Ok(())
}

#[test]
fn create_validates_names_and_copies_bundled_with_rewritten_name() -> Result<(), WorkflowError> {
    let store = TempDir::new("create-from")?;
    for invalid in ["", "Upper", "has/slash", "has\\slash", ".."] {
        assert!(matches!(
            crate::create::create_plan(store.path(), invalid, None),
            Err(WorkflowError::InvalidWorkflowName(_))
        ));
    }

    let plan = crate::create::create_plan(store.path(), "custom-research", Some("deep-research"))?;
    assert_eq!(
        crate::meta::extract(&plan.contents)?.name,
        "custom-research"
    );
    assert!(plan.contents.contains("name: \"custom-research\""));
    assert!(!plan.contents.contains("name: \"deep-research\""));
    Engine::new(EchoHost).compile(&plan.contents)?;
    assert!(matches!(
        crate::create::create_plan(store.path(), "custom", Some("not-official")),
        Err(WorkflowError::BundledWorkflowNotFound(name)) if name == "not-official"
    ));
    Ok(())
}

#[test]
fn bundled_name_rewrite_handles_spacing_and_ignores_string_contents() -> Result<(), WorkflowError> {
    for source in [
        r#"let meta = #{ description: "mentions name: \"odd-format\"", name:"odd-format" };
complete(true);
"#,
        r#"let meta = #{ description: "mentions name: \"odd-format\"", name :  "odd-format" };
complete(true);
"#,
    ] {
        let rewritten =
            crate::create::rewrite_bundled_name(source, "odd-format", "rewritten-name")?;
        let metadata = crate::meta::extract(&rewritten)?;
        assert_eq!(metadata.name, "rewritten-name");
        assert_eq!(metadata.description, "mentions name: \"odd-format\"");
        assert!(rewritten.contains(r#"mentions name: \"odd-format\""#));
    }
    Ok(())
}

#[test]
fn remove_is_validated_and_only_deletes_from_selected_store() -> Result<(), WorkflowError> {
    let project = TempDir::new("remove-project")?;
    let user = TempDir::new("remove-user")?;
    let project_path = write_workflow(project.path(), "removable", "removable")?;
    let user_path = write_workflow(user.path(), "removable", "removable")?;

    let removed = crate::store::remove_from_store("removable", user.path())?;
    assert_eq!(removed, user_path);
    assert!(!removed.exists());
    assert!(project_path.exists());

    let absent = crate::store::remove_from_store("removable", user.path());
    let absent_message = absent
        .as_ref()
        .err()
        .map(ToString::to_string)
        .ok_or_else(|| WorkflowError::InvalidConfig("missing remove error".into()))?;
    assert!(matches!(
        absent,
        Err(WorkflowError::WorkflowNotFound { .. })
    ));
    assert!(absent_message.contains("workflow `removable` is not present in the user store"));
    assert!(!absent_message.contains("skill"));

    let invalid = crate::store::remove_from_store("../removable", user.path());
    let invalid_message = invalid
        .as_ref()
        .err()
        .map(ToString::to_string)
        .ok_or_else(|| WorkflowError::InvalidConfig("missing invalid-name error".into()))?;
    assert!(matches!(
        invalid,
        Err(WorkflowError::InvalidWorkflowName(name)) if name == "../removable"
    ));
    assert_eq!(
        invalid_message,
        "invalid workflow name `../removable` (allowed: lowercase letters, digits, and hyphens)"
    );
    assert!(project_path.exists());
    Ok(())
}
