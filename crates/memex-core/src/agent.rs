//! The agent edge's trait and the retry policy around it. Which library or provider answers is the edge's business.
use memex_types::{AgentError, AgentReply, AgentRequest, AgentUsage};

/// One call to an LLM with no tools and schema-only output. Synchronous, like every trait here, so the async client stays inside its adapter.
pub trait AgentRunner {
    /// Names the model, for Derivations.
    fn model(&self) -> String;
    fn run(&self, req: &AgentRequest) -> Result<AgentReply, AgentError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOutcome {
    pub result: Result<AgentReply, AgentError>,
    pub attempts: u32,
    /// Everything spent, across attempts and including those that failed.
    pub usage: AgentUsage,
    /// A retry was due, but didn't fit in the budget.
    pub out_of_budget: bool,
}

fn checked<R: AgentRunner>(runner: &R, req: &AgentRequest, check: &dyn Fn(&str) -> Result<(), String>) -> Result<AgentReply, AgentError> {
    let reply = runner.run(req)?;
    if reply.tool_calls > 0 {
        return Err(AgentError::SchemaInvalid {
            detail: format!("it tried {} tool calls, and has no tools", reply.tool_calls),
            usage: reply.usage,
        });
    }
    if let Err(detail) = check(&reply.json) {
        return Err(AgentError::SchemaInvalid { detail, usage: reply.usage });
    }
    Ok(reply)
}

fn spent(result: &Result<AgentReply, AgentError>) -> AgentUsage {
    match result {
        Ok(r) => r.usage,
        Err(AgentError::SchemaInvalid { usage, .. }) | Err(AgentError::EmptyReply { usage }) | Err(AgentError::Truncated { usage }) => *usage,
        Err(_) => AgentUsage::default(),
    }
}

/// One retry on invalid, empty or cut-off output, with the reason added to the input. Anything else returns at once. `room_for_retry` gets what's been spent, and answers with the output limit for the retry, or none if it doesn't fit.
pub fn run_with_retry<R: AgentRunner>(
    runner: &R,
    req: &AgentRequest,
    check: &dyn Fn(&str) -> Result<(), String>,
    room_for_retry: &dyn Fn(&AgentUsage) -> Option<u32>,
) -> AgentOutcome {
    let first = checked(runner, req, check);
    let mut usage = spent(&first);
    let reason = match &first {
        Err(AgentError::SchemaInvalid { detail, .. }) => format!("Your previous reply was invalid: {detail}. Reply with only JSON matching the schema."),
        Err(AgentError::EmptyReply { .. }) | Err(AgentError::Truncated { .. }) => {
            "Your previous reply was empty or cut off. Reply briefly with only JSON matching the schema.".to_string()
        }
        _ => {
            return AgentOutcome {
                result: first,
                attempts: 1,
                usage,
                out_of_budget: false,
            };
        }
    };
    let Some(max_output_tokens) = room_for_retry(&usage) else {
        return AgentOutcome {
            result: first,
            attempts: 1,
            usage,
            out_of_budget: true,
        };
    };
    let mut retry = req.clone();
    retry.input = format!("{}\n\n{reason}", req.input);
    retry.budget.max_output_tokens = max_output_tokens;
    let second = checked(runner, &retry, check);
    usage.add(&spent(&second));
    AgentOutcome {
        result: second,
        attempts: 2,
        usage,
        out_of_budget: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ScriptedAgent;
    use memex_types::AgentBudget;

    fn req() -> AgentRequest {
        AgentRequest {
            instruction: "i".into(),
            input: "x".into(),
            schema_json: "{}".into(),
            budget: AgentBudget {
                max_output_tokens: 10,
                timeout_ms: 10,
            },
        }
    }

    fn usage(input: u64, output: u64, thinking: u64) -> AgentUsage {
        AgentUsage {
            input_tokens: input,
            output_tokens: output,
            thinking_tokens: thinking,
        }
    }

    fn ok() -> AgentReply {
        AgentReply {
            json: "{}".into(),
            usage: usage(2, 3, 0),
            tool_calls: 0,
        }
    }

    fn accept(_: &str) -> Result<(), String> {
        Ok(())
    }

    fn room(_: &AgentUsage) -> Option<u32> {
        Some(7)
    }

    #[test]
    fn a_valid_first_reply_uses_one_attempt() {
        let o = run_with_retry(&ScriptedAgent::new(vec![Ok(ok())]), &req(), &accept, &room);
        assert_eq!((o.attempts, o.usage.output_tokens, o.out_of_budget), (1, 3, false));
        assert!(o.result.is_ok());
    }

    #[test]
    fn invalid_output_retries_once_says_why_and_sums_usage() {
        let agent = ScriptedAgent::new(vec![
            Err(AgentError::SchemaInvalid {
                detail: "missing name".into(),
                usage: usage(1, 1, 5),
            }),
            Ok(ok()),
        ]);
        let o = run_with_retry(&agent, &req(), &accept, &room);
        assert_eq!((o.attempts, o.usage.input_tokens, o.usage.thinking_tokens), (2, 3, 5));
        assert!(o.result.is_ok());
        let sent = agent.requests();
        assert!(sent[1].input.contains("missing name"), "{}", sent[1].input);
        assert_eq!(sent[1].budget.max_output_tokens, 7);
    }

    #[test]
    fn the_check_refuses_what_the_runner_passed() {
        let refuse = |_: &str| Err("not a list".to_string());
        let o = run_with_retry(&ScriptedAgent::new(vec![Ok(ok()), Ok(ok())]), &req(), &refuse, &room);
        assert_eq!((o.attempts, o.usage.total()), (2, 10));
        assert!(matches!(o.result, Err(AgentError::SchemaInvalid { .. })));
    }

    #[test]
    fn a_tool_call_is_invalid_even_with_good_json() {
        let mut r = ok();
        r.tool_calls = 1;
        let o = run_with_retry(&ScriptedAgent::new(vec![Ok(r.clone()), Ok(r)]), &req(), &accept, &room);
        assert!(matches!(o.result, Err(AgentError::SchemaInvalid { .. })));
    }

    #[test]
    fn an_empty_reply_stays_typed_after_its_retry() {
        let empty = || Err(AgentError::EmptyReply { usage: usage(1, 1, 5) });
        let o = run_with_retry(&ScriptedAgent::new(vec![empty(), empty()]), &req(), &accept, &room);
        assert_eq!(o.attempts, 2);
        assert!(matches!(o.result, Err(AgentError::EmptyReply { .. })));
    }

    #[test]
    fn an_outage_returns_at_once_and_does_not_retry() {
        for e in [
            AgentError::Unavailable("down".into()),
            AgentError::RateLimited("slow".into()),
            AgentError::Timeout,
        ] {
            let agent = ScriptedAgent::new(vec![Err(e.clone())]);
            let o = run_with_retry(&agent, &req(), &accept, &room);
            assert_eq!((o.attempts, agent.requests().len()), (1, 1));
            assert_eq!(o.result.unwrap_err(), e);
        }
    }

    #[test]
    fn a_retry_that_does_not_fit_is_not_made() {
        let agent = ScriptedAgent::new(vec![Err(AgentError::Truncated { usage: usage(9, 1, 0) })]);
        let o = run_with_retry(&agent, &req(), &accept, &|_| None);
        assert_eq!((o.attempts, o.out_of_budget, o.usage.total()), (1, true, 10));
        assert_eq!(agent.requests().len(), 1);
    }
}
