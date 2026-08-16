use rig::{
    agent::Text,
    memory::MemoryError,
    message::{ReasoningContent, ToolCall},
    prelude::*,
    streaming::ToolCallDeltaContent,
};

pub(crate) mod dynhook;
pub mod orchestrator;
#[cfg(test)]
mod tests;

#[trait_variant::make(Send)]
pub trait Agent {
    async fn send_turn(&mut self, message: impl Into<Message> + Send) -> anyhow::Result<()>;
    async fn load_conversation(&mut self, id: impl AsRef<str> + Send) -> anyhow::Result<()>;
    fn conversation_id(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageUpdate {
    Replace(Message),
    AssistantTextAppend(Text),
    AssistantReasoningAppend(String),
    AssistantReasoningReplace(Vec<ReasoningContent>),
    ToolCallReplace(ToolCall),
    ToolCallAppend {
        id: String,
        content: ToolCallDeltaContent,
    },
}

#[trait_variant::make(Send)]
pub trait AgentLifecycleHook {
    async fn on_switch_conversation(
        &self,
        id: &str,
        memory: Result<Vec<Message>, MemoryError>,
    ) -> anyhow::Result<()>;

    async fn on_new_message(&self, index: usize, message: &Message) -> anyhow::Result<()>;

    fn on_update_message(&self, index: usize, message: &MessageUpdate);
}
