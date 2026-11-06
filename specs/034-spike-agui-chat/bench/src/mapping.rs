//! Pure: runner events in, AG-UI events out, in the order the protocol requires. No ADK, runtime or IO here.
use serde::Serialize;
use serde_json::Value;

/// What a streaming `AgentRunner` would expose. `turn` is stable for one model call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunnerEvent {
    pub turn: String,
    /// A partial event holds only what is new. A terminal one repeats everything so far in this turn.
    pub partial: bool,
    pub parts: Vec<RunnerPart>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerPart {
    Text(String),
    ToolCall { id: Option<String>, name: String, args: Value },
    ToolResult { id: Option<String>, name: String, result: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum AgUi {
    RunStarted { thread_id: String, run_id: String },
    TextMessageStart { message_id: String, role: &'static str },
    TextMessageContent { message_id: String, delta: String },
    TextMessageEnd { message_id: String },
    ToolCallStart { tool_call_id: String, tool_call_name: String, #[serde(skip_serializing_if = "Option::is_none")] parent_message_id: Option<String> },
    ToolCallArgs { tool_call_id: String, delta: String },
    ToolCallEnd { tool_call_id: String },
    ToolCallResult { message_id: String, tool_call_id: String, content: String, role: &'static str },
    RunFinished { thread_id: String, run_id: String },
    RunError { message: String, code: String },
}

pub struct Mapper {
    thread: String,
    run: String,
    open_message: Option<String>,
    last_message: Option<String>,
    ids: u32,
    turn: String,
    turn_text: String,
    turn_calls: usize,
    awaiting_result: Vec<(String, String)>,
    ended: bool,
}

impl Mapper {
    pub fn new(thread: impl Into<String>, run: impl Into<String>) -> Self {
        Self {
            thread: thread.into(), run: run.into(), open_message: None, last_message: None, ids: 0,
            turn: String::new(), turn_text: String::new(), turn_calls: 0, awaiting_result: Vec::new(), ended: false,
        }
    }

    fn id(&mut self, kind: &str) -> String {
        self.ids += 1;
        format!("{}-{kind}{}", self.run, self.ids)
    }

    pub fn start(&self) -> Vec<AgUi> {
        vec![AgUi::RunStarted { thread_id: self.thread.clone(), run_id: self.run.clone() }]
    }

    fn text(&mut self, delta: &str, out: &mut Vec<AgUi>) {
        if delta.is_empty() { return; } // The schema refuses an empty delta.
        self.turn_text.push_str(delta);
        let message_id = match &self.open_message {
            Some(m) => m.clone(),
            None => {
                let m = self.id("m");
                out.push(AgUi::TextMessageStart { message_id: m.clone(), role: "assistant" });
                self.open_message = Some(m.clone());
                self.last_message = Some(m.clone());
                m
            }
        };
        out.push(AgUi::TextMessageContent { message_id, delta: delta.to_string() });
    }

    fn close_text(&mut self, out: &mut Vec<AgUi>) {
        if let Some(message_id) = self.open_message.take() { out.push(AgUi::TextMessageEnd { message_id }); }
    }

    pub fn on_event(&mut self, ev: &RunnerEvent) -> Vec<AgUi> {
        let mut out = Vec::new();
        if self.ended { return out; }
        if ev.turn != self.turn {
            self.close_text(&mut out); // A new model call is a new assistant message.
            self.turn = ev.turn.clone();
            self.turn_text.clear();
            self.turn_calls = 0;
        }
        if !ev.partial {
            // ADK's terminal event repeats the turn's text. Only what never went out as a delta is new.
            let full: String = ev.parts.iter().filter_map(|p| if let RunnerPart::Text(t) = p { Some(t.as_str()) } else { None }).collect();
            if let Some(rest) = full.strip_prefix(self.turn_text.as_str()) {
                let rest = rest.to_string();
                self.text(&rest, &mut out);
            }
        }
        let mut seen_calls = 0;
        for part in &ev.parts {
            match part {
                RunnerPart::Text(t) => if ev.partial { self.text(t, &mut out) },
                RunnerPart::ToolCall { id, name, args } => {
                    seen_calls += 1;
                    if !ev.partial && seen_calls <= self.turn_calls { continue; } // Repeated by the terminal event.
                    self.turn_calls += 1;
                    self.close_text(&mut out);
                    let tool_call_id = id.clone().unwrap_or_else(|| self.id("t")); // ADK documents the id as optional. Gemini gave one in the live runs.
                    out.push(AgUi::ToolCallStart { tool_call_id: tool_call_id.clone(), tool_call_name: name.clone(), parent_message_id: self.last_message.clone() });
                    out.push(AgUi::ToolCallArgs { tool_call_id: tool_call_id.clone(), delta: args.to_string() });
                    out.push(AgUi::ToolCallEnd { tool_call_id: tool_call_id.clone() });
                    self.awaiting_result.push((tool_call_id, name.clone()));
                }
                RunnerPart::ToolResult { id, name, result } => {
                    self.close_text(&mut out);
                    let at = self.awaiting_result.iter().position(|(t, n)| id.as_deref() == Some(t.as_str()) || (id.is_none() && n == name))
                        .or_else(|| self.awaiting_result.iter().position(|(_, n)| n == name));
                    let tool_call_id = match at { Some(i) => self.awaiting_result.remove(i).0, None => self.id("t") };
                    let message_id = self.id("r");
                    out.push(AgUi::ToolCallResult { message_id, tool_call_id, content: result.to_string(), role: "tool" });
                }
            }
        }
        out
    }

    /// A failure ends the run. The protocol allows RUN_ERROR with a message still open, and nothing may follow it.
    pub fn on_error(&mut self, code: &str, message: &str) -> Vec<AgUi> {
        if self.ended { return Vec::new(); }
        self.ended = true;
        vec![AgUi::RunError { message: message.to_string(), code: code.to_string() }]
    }

    pub fn finish(&mut self) -> Vec<AgUi> {
        if self.ended { return Vec::new(); }
        self.ended = true;
        let mut out = Vec::new();
        self.close_text(&mut out);
        out.push(AgUi::RunFinished { thread_id: self.thread.clone(), run_id: self.run.clone() });
        out
    }
}

/// One JSON object per line, for golden comparison.
#[cfg(test)]
pub fn to_jsonl(events: &[AgUi]) -> String {
    events.iter().map(|e| serde_json::to_string(e).expect("serializes") + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use RunnerPart::*;

    fn ev(turn: &str, partial: bool, parts: Vec<RunnerPart>) -> RunnerEvent { RunnerEvent { turn: turn.into(), partial, parts } }

    fn run(events: &[RunnerEvent]) -> Vec<AgUi> {
        let mut m = Mapper::new("th", "r");
        let mut out = m.start();
        for e in events { out.extend(m.on_event(e)); }
        out.extend(m.finish());
        out
    }

    fn delta_text(out: &[AgUi]) -> String {
        out.iter().filter_map(|e| if let AgUi::TextMessageContent { delta, .. } = e { Some(delta.as_str()) } else { None }).collect()
    }

    #[test]
    fn terminal_event_does_not_repeat_streamed_text() {
        let out = run(&[ev("a", true, vec![Text("Hel".into())]), ev("a", true, vec![Text("lo".into())]), ev("a", false, vec![Text("Hel".into()), Text("lo".into())])]);
        assert_eq!(delta_text(&out), "Hello");
    }

    #[test]
    fn text_seen_only_in_the_terminal_event_still_goes_out() {
        assert_eq!(delta_text(&run(&[ev("a", true, vec![Text("Hel".into())]), ev("a", false, vec![Text("Hello!".into())])])), "Hello!");
    }

    #[test]
    fn empty_deltas_are_dropped() {
        let out = run(&[ev("a", true, vec![Text(String::new())])]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn two_equal_calls_in_a_turn_stay_two_and_the_terminal_repeat_adds_none() {
        let call = ToolCall { id: None, name: "f".into(), args: json!({}) };
        let out = run(&[ev("a", true, vec![call.clone()]), ev("a", true, vec![call.clone()]), ev("a", false, vec![call.clone(), call])]);
        assert_eq!(out.iter().filter(|e| matches!(e, AgUi::ToolCallStart { .. })).count(), 2);
    }

    #[test]
    fn result_without_ids_pairs_with_the_first_waiting_call_of_that_name() {
        let call = ToolCall { id: None, name: "f".into(), args: json!({}) };
        let result = ToolResult { id: None, name: "f".into(), result: json!(1) };
        let out = run(&[ev("a", false, vec![call]), ev("b", false, vec![result])]);
        let start = out.iter().find_map(|e| if let AgUi::ToolCallStart { tool_call_id, .. } = e { Some(tool_call_id) } else { None });
        let result = out.iter().find_map(|e| if let AgUi::ToolCallResult { tool_call_id, .. } = e { Some(tool_call_id) } else { None });
        assert_eq!(start, result);
    }

    #[test]
    fn new_turn_closes_the_message_and_starts_another() {
        let out = run(&[ev("a", true, vec![Text("x".into())]), ev("b", true, vec![Text("y".into())])]);
        assert_eq!(out.iter().filter(|e| matches!(e, AgUi::TextMessageStart { .. })).count(), 2);
        assert_eq!(out.iter().filter(|e| matches!(e, AgUi::TextMessageEnd { .. })).count(), 2);
    }

    #[test]
    fn error_is_last_and_nothing_follows() {
        let mut m = Mapper::new("th", "r");
        m.on_event(&ev("a", true, vec![Text("x".into())]));
        assert!(matches!(m.on_error("unavailable", "m")[..], [AgUi::RunError { .. }]));
        assert!(m.finish().is_empty());
        assert!(m.on_event(&ev("a", true, vec![Text("y".into())])).is_empty());
        assert!(m.on_error("x", "y").is_empty());
    }
}
