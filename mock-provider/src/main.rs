use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Json, Router, routing::post};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let port = std::env::var("PORT").unwrap_or_else(|_| "9001".into());
    let addr = format!("0.0.0.0:{port}");

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", axum::routing::get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!("mock-provider listening on {addr}");

    axum::serve(listener, app).await.expect("server error");
}

fn provider_id() -> String {
    let port = std::env::var("PORT").unwrap_or_else(|_| "9001".into());
    format!("mock:{port}")
}

fn token_latency_ms() -> u64 {
    std::env::var("MOCK_LATENCY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
}

fn error_rate() -> f64 {
    std::env::var("MOCK_ERROR_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

#[derive(Deserialize)]
struct ChatRequest {
    model: String,
    #[serde(default)]
    stream: bool,
    #[allow(dead_code)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[allow(dead_code)]
    role: String,
    #[allow(dead_code)]
    content: String,
}

#[derive(Serialize)]
struct ChatResponse {
    id: String,
    object: String,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Serialize)]
struct Choice {
    index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<ResponseMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<ResponseMessage>,
    finish_reason: Option<String>,
}

#[derive(Serialize)]
struct ResponseMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Serialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

async fn chat_completions(Json(req): Json<ChatRequest>) -> impl IntoResponse {
    let count = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Simulate errors based on MOCK_ERROR_RATE
    let err_rate = error_rate();
    if err_rate > 0.0 && (count as f64 % (1.0 / err_rate)) < 1.0 {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "simulated failure"})),
        )
            .into_response();
    }

    if req.stream {
        stream_response(req.model).into_response()
    } else {
        json_response(req.model).into_response()
    }
}

fn request_id() -> String {
    let count = REQUEST_COUNTER.load(Ordering::Relaxed);
    format!("mock-{}-{count}", provider_id())
}

fn json_response(model: String) -> Json<ChatResponse> {
    let pid = provider_id();
    Json(ChatResponse {
        id: request_id(),
        object: "chat.completion".into(),
        model,
        choices: vec![Choice {
            index: 0,
            message: Some(ResponseMessage {
                role: Some("assistant".into()),
                content: Some(format!("Hello from {pid}!")),
            }),
            delta: None,
            finish_reason: Some("stop".into()),
        }],
        usage: Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        },
    })
}

fn stream_response(model: String) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let latency = token_latency_ms();

    let stream = async_stream::stream! {
        let pid = provider_id();
        let tokens: [String; 4] = ["Hello".into(), " from".into(), format!(" {pid}"), "!".into()];
        for (i, token) in tokens.iter().enumerate() {
            tokio::time::sleep(Duration::from_millis(latency)).await;

            let chunk = ChatResponse {
                id: request_id(),
                object: "chat.completion.chunk".into(),
                model: model.clone(),
                choices: vec![Choice {
                    index: 0,
                    message: None,
                    delta: Some(ResponseMessage {
                        role: if i == 0 { Some("assistant".into()) } else { None },
                        content: Some(token.to_string()),
                    }),
                    finish_reason: None,
                }],
                usage: Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                },
            };

            let data = serde_json::to_string(&chunk).unwrap();
            yield Ok(Event::default().data(data));
        }

        let done_chunk = ChatResponse {
            id: request_id(),
            object: "chat.completion.chunk".into(),
            model: model.clone(),
            choices: vec![Choice {
                index: 0,
                message: None,
                delta: Some(ResponseMessage {
                    role: None,
                    content: None,
                }),
                finish_reason: Some("stop".into()),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };

        let data = serde_json::to_string(&done_chunk).unwrap();
        yield Ok(Event::default().data(data));

        yield Ok(Event::default().data("[DONE]"));
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
