use rig::{memory::MemoryError, prelude::*};
use tokio_util::sync::CancellationToken;

pub mod config;
pub(crate) mod dynhook;
pub mod orchestrator;
#[cfg(test)]
mod tests;

pub use yaca_transport::MessageUpdate;

#[trait_variant::make(Send)]
pub trait Agent {
    async fn send_turn(
        &mut self,
        message: impl Into<Message> + Send,
        max_tokens: u64,
        cancel: CancellationToken,
    ) -> anyhow::Result<()>;
    async fn load_conversation(&mut self, id: impl AsRef<str> + Send) -> anyhow::Result<()>;
    fn conversation_id(&self) -> &str;
}

#[trait_variant::make(Send)]
pub trait AgentLifecycleHook {
    async fn on_switch_conversation(
        &self,
        id: &str,
        memory: Result<Vec<Message>, MemoryError>,
    ) -> anyhow::Result<()>;

    async fn on_new_message(&self, index: usize, message: &Message) -> anyhow::Result<()>;

    async fn on_update_message(&self, index: usize, message: &MessageUpdate) -> anyhow::Result<()>;
}
