use std::sync::Arc;

use futures::StreamExt;
use rig::prelude::*;
use tokio_util::sync::CancellationToken;

use crate::agent::orchestrator::{OrchestratorAgent, OrchestratorParams};
use crate::agent::Agent;
use crate::tools::Environment;

/// A completion model whose `stream()` yields a single text chunk and then
/// blocks forever (never terminates), so an in-flight turn can be cancelled.
#[derive(Clone)]
struct BlockingModel {
    started: Arc<tokio::sync::Notify>,
}

impl CompletionModel for BlockingModel {
    async fn completion(
        &self,
        _request: rig::completion::CompletionRequest,
    ) -> Result<rig::completion::CompletionResponse, CompletionError> {
        Err(CompletionError::ResponseError("unary not supported".into()))
    }

    async fn stream(
        &self,
        _request: rig::completion::CompletionRequest,
    ) -> Result<rig::streaming::StreamingCompletionResponse, CompletionError> {
        self.started.notify_one();
        let items: Vec<Result<rig::streaming::RawStreamingChoice, CompletionError>> =
            vec![Ok(rig::streaming::RawStreamingChoice::Message(
                "first chunk".into(),
            ))];
        let stream: rig::streaming::StreamingResult = Box::pin(
            futures::stream::iter(items).chain(futures::stream::pending()),
        );
        Ok(rig::streaming::StreamingCompletionResponse::stream(
            "mock", stream,
        ))
    }
}

#[derive(Clone)]
struct BlockingClient {
    started: Arc<tokio::sync::Notify>,
}

impl CompletionClient for BlockingClient {
    type CompletionModel = BlockingModel;

    fn completion_model(&self, _model: impl Into<String>) -> Self::CompletionModel {
        BlockingModel {
            started: self.started.clone(),
        }
    }
}

#[tokio::test]
async fn cancelling_a_turn_aborts_the_inflight_stream() {
    let started = Arc::new(tokio::sync::Notify::new());
    let params: OrchestratorParams<
        BlockingModel,
        BlockingClient,
        rig::memory::InMemoryConversationMemory,
    > = OrchestratorParams::new(
        Environment::default(),
        BlockingClient {
            started: started.clone(),
        },
        "mock-model",
        rig::memory::InMemoryConversationMemory::new(),
    );
    let mut agent = OrchestratorAgent::new(params, "conv");
    let cancel = CancellationToken::new();
    let handle_cancel = cancel.clone();

    let handle = tokio::spawn(async move {
        Agent::send_turn(&mut agent, Message::user("hi"), 1024, handle_cancel).await
    });

    // Wait until the model's stream has been opened, so the turn is in flight.
    started.notified().await;
    cancel.cancel();

    let result = handle.await.expect("send_turn task panicked");
    let err = result.expect_err("cancelled turn must return an error");
    assert!(
        err.to_string().contains("turn cancelled"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn a_completing_turn_is_not_an_error() {
    // A model whose stream yields one chunk and then terminates normally.
    #[derive(Clone)]
    struct FinishModel;

    impl CompletionModel for FinishModel {
        async fn completion(
            &self,
            _request: rig::completion::CompletionRequest,
        ) -> Result<rig::completion::CompletionResponse, CompletionError> {
            Err(CompletionError::ResponseError("unary not supported".into()))
        }

        async fn stream(
            &self,
            _request: rig::completion::CompletionRequest,
        ) -> Result<rig::streaming::StreamingCompletionResponse, CompletionError> {
            let items: Vec<Result<rig::streaming::RawStreamingChoice, CompletionError>> =
                vec![Ok(rig::streaming::RawStreamingChoice::Message(
                    "done".into(),
                ))];
            let stream: rig::streaming::StreamingResult =
                Box::pin(futures::stream::iter(items));
            Ok(rig::streaming::StreamingCompletionResponse::stream(
                "mock", stream,
            ))
        }
    }

    #[derive(Clone)]
    struct FinishClient;

    impl CompletionClient for FinishClient {
        type CompletionModel = FinishModel;

        fn completion_model(&self, _model: impl Into<String>) -> Self::CompletionModel {
            FinishModel
        }
    }

    let params: OrchestratorParams<
        FinishModel,
        FinishClient,
        rig::memory::InMemoryConversationMemory,
    > = OrchestratorParams::new(
        Environment::default(),
        FinishClient,
        "mock-model",
        rig::memory::InMemoryConversationMemory::new(),
    );
    let mut agent = OrchestratorAgent::new(params, "conv");
    let cancel = CancellationToken::new();

    agent
        .send_turn(Message::user("hi"), 1024, cancel)
        .await
        .expect("a normal turn should complete successfully");
}
