//! An agent for tests, so the machinery around a model runs with no model.
type AgentReplyFn = Box<dyn Fn(&memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> + Send + Sync>;

/// An agent that plays back a script of replies, and then answers with `then` if it has one, or as an outage. It keeps every request. It never stands in for a model's quality: its replies are whatever the test wrote.
pub struct ScriptedAgent {
    script: std::sync::Mutex<std::collections::VecDeque<Result<memex_types::AgentReply, memex_types::AgentError>>>,
    requests: std::sync::Mutex<Vec<memex_types::AgentRequest>>,
    then: Option<AgentReplyFn>,
}

impl ScriptedAgent {
    pub fn new(script: Vec<Result<memex_types::AgentReply, memex_types::AgentError>>) -> Self {
        Self {
            script: std::sync::Mutex::new(script.into()),
            requests: std::sync::Mutex::new(vec![]),
            then: None,
        }
    }

    /// Answers every request by calling `f`.
    pub fn answering(f: impl Fn(&memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> + Send + Sync + 'static) -> Self {
        let mut agent = Self::new(vec![]);
        agent.then = Some(Box::new(f));
        agent
    }

    /// Once the script runs out, answers by calling `f`.
    pub fn then(mut self, f: impl Fn(&memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> + Send + Sync + 'static) -> Self {
        self.then = Some(Box::new(f));
        self
    }

    pub fn requests(&self) -> Vec<memex_types::AgentRequest> {
        self.requests.lock().expect("lock").clone()
    }
}

impl crate::AgentRunner for ScriptedAgent {
    fn model(&self) -> String {
        "scripted".into()
    }

    fn run(&self, req: &memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> {
        self.requests.lock().expect("lock").push(req.clone());
        if let Some(next) = self.script.lock().expect("lock").pop_front() {
            return next;
        }
        match &self.then {
            Some(f) => f(req),
            None => Err(memex_types::AgentError::Unavailable("the script ran out".into())),
        }
    }
}
