//! Sketch of the `AgentRunner` trait for memex-core: plain types, synchronous, no ADK type. Tested here with a fake.

#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub max_output_tokens: u32,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRequest {
    pub instruction: String,
    pub input: String,
    pub schema_json: String,
    pub budget: Budget,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentReply {
    pub json: String,
    pub usage: Usage,
    pub tool_calls: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentError {
    /// Endpoint down or unreachable. The job waits and retries later.
    Unavailable(String),
    RateLimited(String),
    Timeout,
    /// The model returned no text, for example only reasoning.
    EmptyReply { usage: Usage },
    /// The model hit its token limit before finishing.
    Truncated { usage: Usage },
    /// Text came back but doesn't satisfy the schema. Retried once.
    SchemaInvalid { detail: String, usage: Usage },
    Provider(String),
}

pub trait AgentRunner {
    fn run(&self, req: &AgentRequest) -> Result<AgentReply, AgentError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub result: Result<AgentReply, AgentError>,
    pub attempts: u32,
    pub usage: Usage,
}

/// Pure policy: one retry on invalid, empty or truncated output, adding the reason to the input. Everything else returns at once.
pub fn run_with_retry<R: AgentRunner>(runner: &R, req: &AgentRequest) -> Outcome {
    let mut usage = Usage::default();
    let add = |u: &mut Usage, x: &Usage| {
        u.input_tokens += x.input_tokens;
        u.output_tokens += x.output_tokens;
        u.thinking_tokens += x.thinking_tokens;
    };
    let first = runner.run(req);
    let reason = match &first {
        Ok(r) => {
            add(&mut usage, &r.usage);
            return Outcome { result: first, attempts: 1, usage };
        }
        Err(AgentError::SchemaInvalid { detail, usage: u }) => {
            add(&mut usage, u);
            format!("Your previous reply was invalid: {detail}. Reply with only JSON matching the schema.")
        }
        Err(AgentError::EmptyReply { usage: u }) | Err(AgentError::Truncated { usage: u }) => {
            add(&mut usage, u);
            "Your previous reply was empty or cut off. Reply briefly with only JSON matching the schema.".to_string()
        }
        Err(_) => return Outcome { result: first, attempts: 1, usage },
    };
    let mut retry = req.clone();
    retry.input = format!("{}\n\n{reason}", req.input);
    let second = runner.run(&retry);
    match &second {
        Ok(r) => add(&mut usage, &r.usage),
        Err(AgentError::SchemaInvalid { usage: u, .. }) | Err(AgentError::EmptyReply { usage: u }) | Err(AgentError::Truncated { usage: u }) => add(&mut usage, u),
        Err(_) => {}
    }
    Outcome { result: second, attempts: 2, usage }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct ScriptedAgentRunner(RefCell<Vec<Result<AgentReply, AgentError>>>);
    impl AgentRunner for ScriptedAgentRunner {
        fn run(&self, _: &AgentRequest) -> Result<AgentReply, AgentError> {
            self.0.borrow_mut().remove(0)
        }
    }
    fn req() -> AgentRequest {
        AgentRequest { instruction: "i".into(), input: "x".into(), schema_json: "{}".into(), budget: Budget { max_output_tokens: 10, timeout_ms: 10 } }
    }
    fn ok() -> AgentReply {
        AgentReply { json: "{}".into(), usage: Usage { input_tokens: 2, output_tokens: 3, thinking_tokens: 0 }, tool_calls: 0 }
    }
    fn used() -> Usage {
        Usage { input_tokens: 1, output_tokens: 1, thinking_tokens: 5 }
    }

    #[test]
    fn first_try_valid_uses_one_attempt() {
        let o = run_with_retry(&ScriptedAgentRunner(RefCell::new(vec![Ok(ok())])), &req());
        assert_eq!((o.attempts, o.usage.output_tokens), (1, 3));
        assert!(o.result.is_ok());
    }
    #[test]
    fn invalid_retries_once_and_sums_usage() {
        let r = ScriptedAgentRunner(RefCell::new(vec![Err(AgentError::SchemaInvalid { detail: "d".into(), usage: used() }), Ok(ok())]));
        let o = run_with_retry(&r, &req());
        assert_eq!((o.attempts, o.usage.input_tokens, o.usage.thinking_tokens), (2, 3, 5));
        assert!(o.result.is_ok());
    }
    #[test]
    fn empty_reply_from_reasoning_is_typed_after_retry() {
        let r = ScriptedAgentRunner(RefCell::new(vec![Err(AgentError::EmptyReply { usage: used() }), Err(AgentError::EmptyReply { usage: used() })]));
        let o = run_with_retry(&r, &req());
        assert_eq!(o.attempts, 2);
        assert!(matches!(o.result, Err(AgentError::EmptyReply { .. })));
    }
    #[test]
    fn unavailable_does_not_retry() {
        let r = ScriptedAgentRunner(RefCell::new(vec![Err(AgentError::Unavailable("down".into()))]));
        let o = run_with_retry(&r, &req());
        assert_eq!(o.attempts, 1);
        assert!(matches!(o.result, Err(AgentError::Unavailable(_))));
    }
}
