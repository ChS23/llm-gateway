use std::convert::Infallible;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Json, Router, routing::post};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let port = std::env::var("PORT").unwrap_or_else(|_| "9001".into());
    let addr = format!("0.0.0.0:{port}");

    let app = Router::new().route("/v1/chat/completions", post(chat_completions));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("failed to bind");

    tracing::info!("mock-provider listening on {addr}");

    axum::serve(listener, app).await.expect("server error");
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
    if req.stream {
        stream_response(req.model).into_response()
    } else {
        json_response(req.model).into_response()
    }
}

fn json_response(model: String) -> Json<ChatResponse> {
    Json(ChatResponse {
        id: "mock-12345".into(),
        object: "chat.completion".into(),
        model,
        choices: vec![Choice {
            index: 0,
            message: Some(ResponseMessage {
                role: Some("assistant".into()),
                content: Some("Hello from mock provider!".into()),
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
    let tokens = ["Hello", " from", " mock", " provider", "!"];

    let stream = async_stream::stream! {
        for (i, token) in tokens.iter().enumerate() {
            // Имитируем задержку между токенами
            tokio::time::sleep(Duration::from_millis(50)).await;

            let chunk = ChatResponse {
                id: "mock-12345".into(),
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

        // Финальный chunk с finish_reason
        let done_chunk = ChatResponse {
            id: "mock-12345".into(),
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
