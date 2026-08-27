use std::sync::Arc;

use anyhow::Context;
use derive_builder::Builder;
use futures::StreamExt;
use rig::{
    memory::{ConversationMemory, MemoryError},
    prelude::*,
    streaming::StreamedAssistantContent,
};
use tokio::sync::RwLock;

use crate::{
    agent::{MessageUpdate, dynhook},
    logging,
    tools::{Environment, Shell},
};

pub struct OrchestratorAgent<P: Initializer> {
    init: P,
    rig: rig::Agent,
    conversation_id: Arc<str>,
    conversation_len: RwLock<Option<usize>>,
    lifecycle_hook: Option<Box<dyn dynhook::DynAgentLifecycleHook + Send + Sync>>,
}

#[derive(Debug)]
pub struct McpServiceAndTools<S: rmcp::Service<rmcp::RoleClient>>(
    rmcp::service::RunningService<rmcp::RoleClient, S>,
    Vec<rmcp::model::Tool>,
);

pub trait Initializer {
    type Memory: ConversationMemory + 'static;
    type Model: CompletionModel + 'static;
    type Client: CompletionClient<CompletionModel = Self::Model>;
    type McpService: rmcp::service::Service<rmcp::RoleClient>;

    fn get_env(&self) -> Arc<Environment>;
    fn get_client(&self) -> &Self::Client;
    fn get_model(&self) -> &str;
    fn get_conversation_memory(&self) -> Self::Memory;

    fn get_mcp_services_and_tools(
        &self,
    ) -> impl Iterator<Item = &McpServiceAndTools<Self::McpService>>;

    fn new_agent(&self) -> rig::Agent {
        let mut client = self
            .get_client()
            .agent(self.get_model())
            .memory(self.get_conversation_memory())
            .tool(Shell::os_default(self.get_env()));
        for McpServiceAndTools(service, tools) in self.get_mcp_services_and_tools() {
            for tool in tools {
                logging::trace!(
                    "{:?}: {}",
                    tool.name.as_ref(),
                    tool.description.as_deref().unwrap_or_default()
                );
            }
            client = client.rmcp_tools(
                tools.into_iter().cloned().collect(),
                service.peer().to_owned(),
            );
        }
        client.build()
    }
}

#[derive(Debug, Builder)]
pub struct OrchestratorParams<
    Model: CompletionModel,
    Client: CompletionClient<CompletionModel = Model>,
    Memory: ConversationMemory,
> {
    pub client: Client,
    #[builder(setter(into))]
    pub env: Arc<Environment>,
    #[builder(setter(into))]
    pub model_name: Arc<str>,
    #[builder(setter(into))]
    pub memory: Arc<Memory>,
    #[builder(private, default)]
    pub mcp_services_tools: Arc<[McpServiceAndTools<rmcp::model::ClientInfo>]>,
}

impl<
    Model: CompletionModel + 'static,
    Client: CompletionClient<CompletionModel = Model>,
    Memory: ConversationMemory + 'static,
> Initializer for OrchestratorParams<Model, Client, Memory>
{
    type Memory = Arc<Memory>;

    type Model = Model;

    type Client = Client;

    type McpService = rmcp::model::ClientInfo;

    fn get_client(&self) -> &Self::Client {
        &self.client
    }

    fn get_model(&self) -> &str {
        &self.model_name
    }

    fn get_conversation_memory(&self) -> Self::Memory {
        Arc::clone(&self.memory)
    }

    fn get_env(&self) -> Arc<Environment> {
        Arc::clone(&self.env)
    }

    fn get_mcp_services_and_tools(
        &self,
    ) -> impl Iterator<Item = &McpServiceAndTools<Self::McpService>> {
        self.mcp_services_tools.iter()
    }
}

impl<
    Model: CompletionModel + 'static,
    Client: CompletionClient<CompletionModel = Model>,
    Memory: ConversationMemory + 'static,
> OrchestratorParams<Model, Client, Memory>
{
    pub fn new(
        env: impl Into<Arc<Environment>>,
        client: Client,
        model: impl Into<Arc<str>>,
        memory: impl Into<Memory>,
    ) -> Self {
        Self {
            env: env.into(),
            client,
            model_name: model.into(),
            memory: Arc::new(memory.into()),
            mcp_services_tools: Default::default(),
        }
    }

    pub async fn with_mcp_servers(
        mut self,
        transport: impl IntoIterator<Item = impl rmcp::transport::Transport<rmcp::RoleClient> + 'static>,
        tools_filter: impl FnMut(&rmcp::model::Tool) -> bool + Clone,
    ) -> anyhow::Result<Self> {
        use rmcp::ServiceExt;
        let client_info = crate::tools::mcp::get_client_info();
        let pairs = futures::future::try_join_all(transport.into_iter().map(
            async |transport| -> anyhow::Result<McpServiceAndTools<_>> {
                let client = client_info
                    .clone()
                    .serve(transport)
                    .await
                    .with_context(|| "connection error")?;
                let tools: Vec<_> = client
                    .list_tools(Default::default())
                    .await
                    .with_context(|| "failed listing tools")?
                    .tools
                    .into_iter()
                    .filter(tools_filter.clone())
                    .collect();
                Ok(McpServiceAndTools(client, tools))
            },
        ))
        .await?;

        self.mcp_services_tools = pairs.into();
        Ok(self)
    }
}

impl<P: Initializer> OrchestratorAgent<P> {
    pub fn new(params: P, conversation_id: impl AsRef<str>) -> Self {
        let rig = params.new_agent();
        Self {
            init: params,
            rig,
            conversation_id: conversation_id.as_ref().into(),
            conversation_len: RwLock::new(None),
            lifecycle_hook: None,
        }
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub async fn snapshot(&self) -> Result<Vec<Message>, MemoryError> {
        self.init
            .get_conversation_memory()
            .load(self.conversation_id())
            .await
    }
}

impl<P> OrchestratorAgent<P>
where
    P: Initializer + Send + Sync,
{
    pub async fn with_lifecycle_hook(
        mut self,
        hook: impl super::AgentLifecycleHook + Send + Sync + 'static,
    ) -> Self {
        let switch_hook = hook
            .on_switch_conversation(
                self.conversation_id(),
                self.init
                    .get_conversation_memory()
                    .load(self.conversation_id())
                    .await,
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
        self.init
            .get_conversation_memory()
            .load(self.conversation_id())
            .await
            .map(|m| m.len())
    }
}

impl<P> super::Agent for OrchestratorAgent<P>
where
    P: Initializer + Send + Sync,
{
    async fn send_turn(
        &mut self,
        message: impl Into<Message> + Send,
        max_tokens: u64,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
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
            .max_turns(usize::MAX)
            .max_tokens(max_tokens)
            .await;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(anyhow::anyhow!("turn cancelled"));
                }
                item = stream.next() => {
                    let Some(item) = item else { break };
                    let Some(hook) = self.lifecycle_hook.as_ref() else {
                        continue;
                    };

                    match item.with_context(|| "prompt stream")? {
                        MultiTurnStreamItem::StreamAssistantItem(assistant) => {
                            let Some(assistant) = streamed_to_update(assistant) else {
                                continue;
                            };
                            hook.on_update_message(new_message_idx, &assistant)
                                .await
                                .with_context(|| "lifecycle hook on_update_message prompt stream")?;
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    async fn load_conversation(&mut self, id: impl AsRef<str> + Send) -> anyhow::Result<()> {
        self.conversation_id = id.as_ref().into();
        if let Some(hook) = self.lifecycle_hook.as_ref() {
            let messages = self
                .init
                .get_conversation_memory()
                .load(self.conversation_id())
                .await;
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

fn streamed_to_update(value: StreamedAssistantContent) -> Option<MessageUpdate> {
    match value {
        StreamedAssistantContent::Text(text) => Some(MessageUpdate::AssistantTextAppend(text)),
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id: _,
        } => Some(MessageUpdate::ToolCallReplace(tool_call)),
        StreamedAssistantContent::ToolCallDelta {
            internal_call_id: id,
            content,
        } => Some(MessageUpdate::ToolCallAppend { id, content }),
        StreamedAssistantContent::Reasoning { reasoning, id: _ } => {
            Some(MessageUpdate::AssistantReasoningReplace(reasoning.content))
        }
        StreamedAssistantContent::ReasoningDelta {
            id: _,
            provider_id: _,
            reasoning,
        } => Some(MessageUpdate::AssistantReasoningAppend(reasoning)),
        _ => None,
    }
}
