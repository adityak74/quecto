use quecto_agent::{
    ApprovalSection, AssistantMessage, CompletionTelemetry, Flavor, HttpModel, Message, Model,
    Provider, ToolCall, ToolsSection, VerifySection,
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
        provider: Provider::OpenAiCompatible,
        max_tokens: None,
    };
    let message = LegacyModel.complete(&[Message::user("hello")], &[]).unwrap();
    let transcript_message = Message {
        role: "assistant".into(),
        content: "legacy response".into(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        reasoning_content: None,
    };
    let flavor = Flavor {
        name: Some("legacy".into()),
        model: Some("legacy-model".into()),
        base_url: None,
        provider: None,
        max_tokens: None,
        max_steps: Some(10),
        auto_verify: None,
        auto_approve: None,
        system_prompt: None,
        system_prompt_file: None,
        tools: ToolsSection::default(),
        approval: ApprovalSection::default(),
        verify: VerifySection::default(),
    };
    let telemetry = CompletionTelemetry::default();

    assert_eq!(model.model, "legacy-model");
    assert_eq!(message.content, "legacy");
    assert_eq!(transcript_message.content, "legacy response");
    assert_eq!(flavor.name.as_deref(), Some("legacy"));
    assert_eq!(telemetry, CompletionTelemetry::default());
}
