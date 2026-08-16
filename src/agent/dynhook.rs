use std::pin::Pin;

use rig::{
    agent::Text,
    memory::MemoryError,
    message::{AssistantContent, ReasoningContent, ToolCall},
    prelude::*,
    streaming::ToolCallDeltaContent,
};

use crate::agent::{AgentLifecycleHook, MessageUpdate};

pub trait DynAgentLifecycleHook {
    fn on_switch_conversation<'s>(
        &'s self,
        id: &'s str,
        memory: Result<Vec<Message>, MemoryError>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 's>>;

    fn on_new_message<'s>(
        &'s self,
        index: usize,
        message: &'s Message,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 's>>;

    fn on_update_message(&self, index: usize, message: &MessageUpdate);
}

impl<T> DynAgentLifecycleHook for T
where
    T: AgentLifecycleHook + Sync,
{
    fn on_switch_conversation<'s>(
        &'s self,
        id: &'s str,
        memory: Result<Vec<Message>, MemoryError>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 's>> {
        Box::pin(<Self as AgentLifecycleHook>::on_switch_conversation(
            self, id, memory,
        ))
    }

    fn on_new_message<'s>(
        &'s self,
        index: usize,
        message: &'s Message,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 's>> {
        Box::pin(<Self as AgentLifecycleHook>::on_new_message(
            self, index, message,
        ))
    }

    fn on_update_message(&self, index: usize, message: &MessageUpdate) {
        <Self as AgentLifecycleHook>::on_update_message(self, index, message)
    }
}
