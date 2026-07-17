use quecto_agent::{
    AssistantMessage, CompletionTelemetry, HttpModel, Message, Model, ToolCall,
};

#[derive(Clone)]
struct LegacyModel;

impl Model for LegacyModel {
    fn complete(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Result<AssistantMessage, quecto_agent::BoxErr> {
        Ok(AssistantMessage {
            content: "legacy".into(),
            tool_calls: Vec::<ToolCall>::new(),
            finish_reason: "stop".into(),
            reasoning_content: None,
        })
    }

    fn clone_box(&self) -> Box<dyn Model> {
        Box::new(self.clone())
    }
}

#[test]
fn legacy_public_struct_literals_still_compile_and_work() {
    let model = HttpModel {
        url: "http://127.0.0.1:1/v1/chat/completions".into(),
        api_key: None,
        model: "legacy-model".into(),
    };
    let message = LegacyModel.complete(&[Message::user("hello")], &[]).unwrap();
    let telemetry = CompletionTelemetry::default();

    assert_eq!(model.model, "legacy-model");
    assert_eq!(message.content, "legacy");
    assert_eq!(telemetry, CompletionTelemetry::default());
}
