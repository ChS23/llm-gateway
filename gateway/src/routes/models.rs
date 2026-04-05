use axum::Json;
use axum::extract::State;
use utoipa::ToSchema;

use crate::state::SharedState;

#[derive(serde::Serialize, ToSchema)]
pub struct ModelsResponse {
    pub object: &'static str,
    pub data: Vec<ModelObject>,
}

#[derive(serde::Serialize, ToSchema)]
pub struct ModelObject {
    /// Model identifier.
    #[schema(example = "gpt-4o")]
    pub id: String,
    pub object: &'static str,
    /// Provider that serves this model.
    #[schema(example = "openai-primary")]
    pub owned_by: String,
}

/// List all available models across all active providers.
#[utoipa::path(
    get,
    path = "/v1/models",
    tag = "LLM Proxy",
    summary = "List available models",
    description = "Returns all models currently available for routing, grouped by provider.",
    responses(
        (status = 200, description = "Model list", body = ModelsResponse),
    ),
    security(("bearer" = []))
)]
pub async fn list_models(State(state): State<SharedState>) -> Json<ModelsResponse> {
    let router = state.router();
    let mut data = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for model in router.available_models() {
        if seen.insert(model.to_string()) {
            // Find which provider serves this model
            let owner = if let Some(provider) = router.resolve(model).await {
                provider.name().to_string()
            } else {
                "unknown".to_string()
            };

            data.push(ModelObject {
                id: model.to_string(),
                object: "model",
                owned_by: owner,
            });
        }
    }

    Json(ModelsResponse {
        object: "list",
        data,
    })
}
