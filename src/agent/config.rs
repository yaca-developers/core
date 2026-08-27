use std::sync::Arc;

use anyhow::Context;
use rig::{memory::InMemoryConversationMemory, providers::openrouter};

use crate::{
    agent::orchestrator::OrchestratorParams,
    tools::Environment,
};

pub type OpenRouterParams = OrchestratorParams<
    openrouter::CompletionModel,
    openrouter::Client,
    InMemoryConversationMemory,
>;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub provider_type: ProviderType,
    pub api_key_env: String,
    pub model: String,
    pub max_tokens_default: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    OpenRouter,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider_type: ProviderType::OpenRouter,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            model: "opus-5".to_string(),
            max_tokens_default: 32_000,
        }
    }
}

impl AgentConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            provider_type: ProviderType::OpenRouter,
            api_key_env: std::env::var("YACA_API_KEY_ENV")
                .unwrap_or_else(|_| "OPENROUTER_API_KEY".to_string()),
            model: std::env::var("YACA_MODEL").unwrap_or_else(|_| "opus-5".to_string()),
            max_tokens_default: std::env::var("YACA_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(32_000),
        })
    }

    pub fn build_openrouter_params(&self) -> anyhow::Result<OpenRouterParams> {
        match self.provider_type {
            ProviderType::OpenRouter => {
                let api_key = std::env::var(&self.api_key_env)
                    .with_context(|| format!("resolving {} for provider", self.api_key_env))?;
                let client = openrouter::Client::new(api_key)
                    .map_err(|e| anyhow::anyhow!("creating OpenRouter client: {e}"))?;
                Ok(OrchestratorParams::new(
                    Environment::default(),
                    client,
                    Arc::<str>::from(self.model.as_str()),
                    InMemoryConversationMemory::new(),
                ))
            }
        }
    }
}
