use std::sync::Arc;

use anyhow::Context;
use futures::StreamExt;
use rig::{
    memory::{ConversationMemory, MemoryError},
    prelude::*,
    streaming::StreamedAssistantContent,
};
use tokio::sync::RwLock;

use crate::{
    agent::{Agent, MessageUpdate, dynhook},
    logging,
    tools::{Environment, Shell},
};

pub struct OrchestratorAgent<M>
where
    M: CompletionModel,
{
    env: Arc<Environment>,
    memory: Arc<dyn ConversationMemory>,
    rig: rig::Agent<M>,
    conversation_id: Arc<str>,
    conversation_len: RwLock<Option<usize>>,
    lifecycle_hook: Option<Box<dyn dynhook::DynAgentLifecycleHook + Send + Sync>>,
}

impl<M> OrchestratorAgent<M>
where
    M: CompletionModel,
{
    pub fn new(
        env: impl Into<Arc<Environment>>,
        client: impl CompletionClient<CompletionModel = M>,
        model: impl Into<String>,
        memory: impl ConversationMemory + 'static,
        conversation_id: impl AsRef<str>,
    ) -> Self {
        let env = env.into();
        let memory = Arc::new(memory);
        let rig = client
            .agent(model)
            .memory(Arc::clone(&memory))
            .tool(Shell::os_default(Arc::clone(&env)))
            .build();
        Self {
            env,
            memory,
            rig,
            conversation_id: conversation_id.as_ref().into(),
            conversation_len: RwLock::new(None),
            lifecycle_hook: None,
        }
    }
}

impl<M> OrchestratorAgent<M>
where
    M: CompletionModel + 'static,
{
    pub async fn with_lifecycle_hook(
        mut self,
        hook: impl super::AgentLifecycleHook + Send + Sync + 'static,
    ) -> Self {
        use super::Agent;
        let switch_hook = hook
            .on_switch_conversation(
                self.conversation_id(),
                self.memory.load(self.conversation_id()).await,
            )
            .await;
        if let Err(err) = switch_hook {
            logging::error!("failed to switch conversation: {err}");
        }
        self.lifecycle_hook = Some(Box::new(hook));
        self
    }

    async fn conversation_len(&self) -> Result<usize, MemoryError> {
        if let Some(len) = self.conversation_len.read().await.as_ref() {
            return Ok(*len);
        }
        self.memory
            .load(self.conversation_id())
            .await
            .map(|m| m.len())
    }
}

impl<M> super::Agent for OrchestratorAgent<M>
where
    M: CompletionModel + 'static,
{
    async fn send_turn(&mut self, message: impl Into<Message> + Send) -> anyhow::Result<()> {
        let message = message.into();
        let new_message_idx = self
            .conversation_len()
            .await
            .with_context(|| "lifecycle hook on_new_message prelude")?;
        if let Some(hook) = self.lifecycle_hook.as_ref() {
            hook.on_new_message(new_message_idx, &message)
                .await
                .map_err(|err| err.context("lifecycle hook on_new_message"))?;
        }
        *self.conversation_len.write().await = Some(new_message_idx + 1);

        let mut stream = self
            .rig
            .stream_prompt(message)
            .conversation(self.conversation_id.to_string())
            .await;
        while let Some(item) = stream.next().await {
            let Some(hook) = self.lifecycle_hook.as_ref() else {
                continue;
            };

            match item.with_context(|| "prompt stream")? {
                MultiTurnStreamItem::StreamAssistantItem(assistant) => {
                    let Some(assistant) = streamed_to_update::<M>(assistant) else {
                        continue;
                    };
                    hook.on_update_message(new_message_idx, &assistant)
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn load_conversation(&mut self, id: impl AsRef<str> + Send) -> anyhow::Result<()> {
        self.conversation_id = id.as_ref().into();
        if let Some(hook) = self.lifecycle_hook.as_ref() {
            let messages = self.memory.load(self.conversation_id()).await;
            if let Ok(messages) = messages.as_ref() {
                *self.conversation_len.write().await = Some(messages.len());
            }
            hook.on_switch_conversation(self.conversation_id(), messages)
                .await
                .map_err(|err| err.context("lifecycle hook on_switch_conversation"))?;
        }
        Ok(())
    }

    fn conversation_id(&self) -> &str {
        self.conversation_id.as_ref()
    }
}

fn streamed_to_update<M: CompletionModel>(
    value: StreamedAssistantContent<M::StreamingResponse>,
) -> Option<MessageUpdate> {
    match value {
        StreamedAssistantContent::Text(text) => Some(MessageUpdate::AssistantTextAppend(text)),
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id: _,
        } => Some(MessageUpdate::ToolCallReplace(tool_call)),
        StreamedAssistantContent::ToolCallDelta {
            id,
            internal_call_id: _,
            content,
        } => Some(MessageUpdate::ToolCallAppend { id, content }),
        StreamedAssistantContent::Reasoning(reasoning) => {
            Some(MessageUpdate::AssistantReasoningReplace(reasoning.content))
        }
        StreamedAssistantContent::ReasoningDelta { id: _, reasoning } => {
            Some(MessageUpdate::AssistantReasoningAppend(reasoning))
        }
        _ => None,
    }
}
