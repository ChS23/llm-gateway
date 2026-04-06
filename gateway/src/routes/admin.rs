use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use rand::Rng;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::agent::{Agent, CreateAgent, UpdateAgent};
use crate::models::provider::{CreateProvider, Provider, UpdateProvider};
use crate::state::SharedState;
use crate::types::GatewayError;

async fn reload_router(state: &SharedState) {
    if let Err(e) = state.reload_router().await {
        tracing::error!(error = %e, "failed to reload router after provider change");
    }
}

// -- Providers ----------------------------------------------------------------

/// Register a new LLM provider backend.
#[utoipa::path(
    post,
    path = "/admin/providers",
    tag = "Providers",
    summary = "Create provider",
    description = "Register a new LLM provider. The gateway router is reloaded automatically.",
    request_body(content = CreateProvider, description = "Provider configuration"),
    responses(
        (status = 201, description = "Provider created", body = Provider),
        (status = 400, description = "Validation error", body = GatewayError),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 500, description = "Internal error", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn create_provider(
    State(state): State<SharedState>,
    Json(input): Json<CreateProvider>,
) -> Result<impl IntoResponse, GatewayError> {
    let models_json = serde_json::to_value(&input.models).unwrap();

    // Encrypt provider API key if provided
    let encrypted_key: Option<Vec<u8>> = input
        .api_key
        .as_deref()
        .and_then(|k| if k.is_empty() { None } else { Some(k) })
        .and_then(|key| {
            crate::crypto::encrypt(key).or_else(|| {
                tracing::warn!("ENCRYPTION_KEY not set, provider API key will not be stored");
                None
            })
        });

    let provider = sqlx::query_as::<_, Provider>(
        r#"
        INSERT INTO providers (name, provider_type, base_url, api_key_encrypted, models,
                               cost_per_input_token, cost_per_output_token, rate_limit_rpm,
                               priority, weight)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING *
        "#,
    )
    .bind(&input.name)
    .bind(&input.provider_type)
    .bind(&input.base_url)
    .bind(&encrypted_key)
    .bind(&models_json)
    .bind(input.cost_per_input_token)
    .bind(input.cost_per_output_token)
    .bind(input.rate_limit_rpm)
    .bind(input.priority)
    .bind(input.weight)
    .fetch_one(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?;

    reload_router(&state).await;
    Ok((StatusCode::CREATED, Json(provider)))
}

/// List all active providers.
#[utoipa::path(
    get,
    path = "/admin/providers",
    tag = "Providers",
    summary = "List providers",
    description = "Returns all active provider backends ordered by name.",
    responses(
        (status = 200, description = "Provider list", body = Vec<Provider>),
        (status = 401, description = "Unauthorized", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn list_providers(
    State(state): State<SharedState>,
) -> Result<Json<Vec<Provider>>, GatewayError> {
    let providers = sqlx::query_as::<_, Provider>(
        "SELECT * FROM providers WHERE is_active = true ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?;

    Ok(Json(providers))
}

/// Get a single provider by ID.
#[utoipa::path(
    get,
    path = "/admin/providers/{id}",
    tag = "Providers",
    summary = "Get provider",
    description = "Fetch a provider by its UUID.",
    params(("id" = Uuid, Path, description = "Provider UUID")),
    responses(
        (status = 200, description = "Provider found", body = Provider),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "Provider not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn get_provider(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Provider>, GatewayError> {
    let provider = sqlx::query_as::<_, Provider>("SELECT * FROM providers WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| GatewayError::internal(e.to_string()))?
        .ok_or_else(|| GatewayError::not_found("provider not found"))?;

    Ok(Json(provider))
}

/// Update an existing provider (partial update).
#[utoipa::path(
    put,
    path = "/admin/providers/{id}",
    tag = "Providers",
    summary = "Update provider",
    description = "Partially update a provider. Only supplied fields are changed. Router is reloaded.",
    params(("id" = Uuid, Path, description = "Provider UUID")),
    request_body(content = UpdateProvider, description = "Fields to update"),
    responses(
        (status = 200, description = "Provider updated", body = Provider),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "Provider not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn update_provider(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateProvider>,
) -> Result<Json<Provider>, GatewayError> {
    let provider = sqlx::query_as::<_, Provider>(
        r#"
        UPDATE providers SET
            base_url = COALESCE($2, base_url),
            models = COALESCE($3, models),
            cost_per_input_token = COALESCE($4, cost_per_input_token),
            cost_per_output_token = COALESCE($5, cost_per_output_token),
            rate_limit_rpm = COALESCE($6, rate_limit_rpm),
            priority = COALESCE($7, priority),
            weight = COALESCE($8, weight),
            is_active = COALESCE($9, is_active),
            updated_at = now()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(&input.base_url)
    .bind(
        input
            .models
            .as_ref()
            .map(|m| serde_json::to_value(m).unwrap()),
    )
    .bind(input.cost_per_input_token)
    .bind(input.cost_per_output_token)
    .bind(input.rate_limit_rpm)
    .bind(input.priority)
    .bind(input.weight)
    .bind(input.is_active)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?
    .ok_or_else(|| GatewayError::not_found("provider not found"))?;

    reload_router(&state).await;
    Ok(Json(provider))
}

/// Soft-delete a provider (sets `is_active = false`).
#[utoipa::path(
    delete,
    path = "/admin/providers/{id}",
    tag = "Providers",
    summary = "Delete provider",
    description = "Soft-delete a provider by setting it inactive. Router is reloaded.",
    params(("id" = Uuid, Path, description = "Provider UUID")),
    responses(
        (status = 204, description = "Provider deleted"),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "Provider not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn delete_provider(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    let result =
        sqlx::query("UPDATE providers SET is_active = false, updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| GatewayError::internal(e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(GatewayError::not_found("provider not found"));
    }

    reload_router(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

// -- Agents -------------------------------------------------------------------

/// Register a new A2A agent.
#[utoipa::path(
    post,
    path = "/admin/agents",
    tag = "Agents",
    summary = "Create agent",
    description = "Register a new A2A agent card. At least one skill is required.",
    request_body(content = CreateAgent, description = "A2A agent card"),
    responses(
        (status = 201, description = "Agent created", body = Agent),
        (status = 400, description = "Validation error", body = GatewayError),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 500, description = "Internal error", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn create_agent(
    State(state): State<SharedState>,
    Json(input): Json<CreateAgent>,
) -> Result<impl IntoResponse, GatewayError> {
    if input.skills.is_empty() {
        return Err(GatewayError::bad_request(
            "validation_error",
            "at least one skill is required",
        ));
    }

    // Store the full card as-is for A2A discovery
    let card_json = serde_json::to_value(&input).unwrap();
    let skills_json = serde_json::to_value(&input.skills).unwrap();

    let agent = sqlx::query_as::<_, Agent>(
        r#"
        INSERT INTO agents (name, description, url, version, provider, capabilities,
                            default_input_modes, default_output_modes, skills, security, card_json)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING *
        "#,
    )
    .bind(&input.name)
    .bind(&input.description)
    .bind(&input.url)
    .bind(&input.version)
    .bind(&input.provider)
    .bind(&input.capabilities)
    .bind(&input.default_input_modes)
    .bind(&input.default_output_modes)
    .bind(&skills_json)
    .bind(&input.security)
    .bind(&card_json)
    .fetch_one(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(agent)))
}

/// List all active agents.
#[utoipa::path(
    get,
    path = "/admin/agents",
    tag = "Agents",
    summary = "List agents",
    description = "Returns all active A2A agents ordered by name.",
    responses(
        (status = 200, description = "Agent list", body = Vec<Agent>),
        (status = 401, description = "Unauthorized", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn list_agents(
    State(state): State<SharedState>,
) -> Result<Json<Vec<Agent>>, GatewayError> {
    let agents =
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE is_active = true ORDER BY name")
            .fetch_all(&state.db)
            .await
            .map_err(|e| GatewayError::internal(e.to_string()))?;

    Ok(Json(agents))
}

/// Get a single agent by ID.
#[utoipa::path(
    get,
    path = "/admin/agents/{id}",
    tag = "Agents",
    summary = "Get agent",
    description = "Fetch an agent by its UUID.",
    params(("id" = Uuid, Path, description = "Agent UUID")),
    responses(
        (status = 200, description = "Agent found", body = Agent),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "Agent not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn get_agent(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Agent>, GatewayError> {
    let agent = sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| GatewayError::internal(e.to_string()))?
        .ok_or_else(|| GatewayError::not_found("agent not found"))?;

    Ok(Json(agent))
}

/// A2A discovery endpoint — returns the agent card JSON as-is.
#[utoipa::path(
    get,
    path = "/admin/agents/{id}/.well-known/agent-card.json",
    tag = "Agents",
    summary = "Get A2A agent card",
    description = "Returns the full A2A agent card JSON for discovery (per A2A Protocol v1.0 spec).",
    params(("id" = Uuid, Path, description = "Agent UUID")),
    responses(
        (status = 200, description = "Agent card JSON", body = Object),
        (status = 404, description = "Agent not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn get_agent_card(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let card: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT card_json FROM agents WHERE id = $1 AND is_active = true")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| GatewayError::internal(e.to_string()))?;

    let (card_json,) = card.ok_or_else(|| GatewayError::not_found("agent not found"))?;
    Ok(Json(card_json))
}

/// Update an existing agent (partial update).
#[utoipa::path(
    put,
    path = "/admin/agents/{id}",
    tag = "Agents",
    summary = "Update agent",
    description = "Partially update an agent. Only supplied fields are changed.",
    params(("id" = Uuid, Path, description = "Agent UUID")),
    request_body(content = UpdateAgent, description = "Fields to update"),
    responses(
        (status = 200, description = "Agent updated", body = Agent),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "Agent not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn update_agent(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateAgent>,
) -> Result<Json<Agent>, GatewayError> {
    let agent = sqlx::query_as::<_, Agent>(
        r#"
        UPDATE agents SET
            description = COALESCE($2, description),
            url = COALESCE($3, url),
            version = COALESCE($4, version),
            provider = COALESCE($5, provider),
            capabilities = COALESCE($6, capabilities),
            skills = COALESCE($7, skills),
            security = COALESCE($8, security),
            is_active = COALESCE($9, is_active),
            updated_at = now()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(&input.description)
    .bind(&input.url)
    .bind(&input.version)
    .bind(&input.provider)
    .bind(&input.capabilities)
    .bind(
        input
            .skills
            .as_ref()
            .map(|s| serde_json::to_value(s).unwrap()),
    )
    .bind(&input.security)
    .bind(input.is_active)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?
    .ok_or_else(|| GatewayError::not_found("agent not found"))?;

    Ok(Json(agent))
}

/// Soft-delete an agent (sets `is_active = false`).
#[utoipa::path(
    delete,
    path = "/admin/agents/{id}",
    tag = "Agents",
    summary = "Delete agent",
    description = "Soft-delete an agent by setting it inactive.",
    params(("id" = Uuid, Path, description = "Agent UUID")),
    responses(
        (status = 204, description = "Agent deleted"),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "Agent not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn delete_agent(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    let result =
        sqlx::query("UPDATE agents SET is_active = false, updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| GatewayError::internal(e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(GatewayError::not_found("agent not found"));
    }

    Ok(StatusCode::NO_CONTENT)
}

// -- API Keys -----------------------------------------------------------------

/// Request body to create a new API key.
#[derive(serde::Deserialize, ToSchema)]
#[schema(example = json!({
    "name": "my-service",
    "scopes": ["chat", "admin"],
    "rate_limit_rpm": 120
}))]
pub struct CreateApiKey {
    /// Human-readable key name.
    pub name: String,
    /// Optional agent ID to scope the key.
    #[serde(default)]
    pub agent_id: Option<Uuid>,
    /// Permission scopes (defaults to `["chat"]`).
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    /// Per-key rate limit in requests per minute (default 60).
    #[serde(default = "default_rate_limit")]
    pub rate_limit_rpm: i32,
}

fn default_scopes() -> Vec<String> {
    vec!["chat".into()]
}

fn default_rate_limit() -> i32 {
    60
}

/// Generate a new API key. The raw key is returned ONCE — only the hash is stored.
#[utoipa::path(
    post,
    path = "/admin/keys",
    tag = "API Keys",
    summary = "Create API key",
    description = "Generate a new `sk-gw-...` API key. The raw key is returned **once** in the response \
                   and is never stored — save it immediately.",
    request_body(content = CreateApiKey, description = "Key configuration"),
    responses(
        (status = 201, description = "API key created (raw key included)", body = Object,
         example = json!({
             "key": "sk-gw-abc123...",
             "key_prefix": "sk-gw-abc123",
             "name": "my-service",
             "scopes": ["chat"],
             "warning": "save this key — it will not be shown again"
         })),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 500, description = "Internal error", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn create_api_key(
    State(state): State<SharedState>,
    Json(input): Json<CreateApiKey>,
) -> Result<impl IntoResponse, GatewayError> {
    let random_bytes: [u8; 16] = rand::rng().random();
    let raw_key = format!(
        "{}-{}",
        state.config.auth.key_prefix,
        hex::encode(random_bytes)
    );
    let key_prefix = &raw_key[..12];
    let key_hash = hex::encode(Sha256::digest(raw_key.as_bytes()));
    let scopes_json = serde_json::to_value(&input.scopes).unwrap();

    sqlx::query(
        r#"
        INSERT INTO api_keys (key_prefix, key_hash, name, agent_id, scopes, rate_limit_rpm)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(key_prefix)
    .bind(&key_hash)
    .bind(&input.name)
    .bind(input.agent_id)
    .bind(&scopes_json)
    .bind(input.rate_limit_rpm)
    .execute(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "key": raw_key,
            "key_prefix": key_prefix,
            "name": input.name,
            "scopes": input.scopes,
            "warning": "save this key — it will not be shown again"
        })),
    ))
}

/// List all API keys (prefix and metadata only — hashes are never exposed).
#[utoipa::path(
    get,
    path = "/admin/keys",
    tag = "API Keys",
    summary = "List API keys",
    description = "Returns all API keys with prefix, name, scopes, and active status. \
                   The full key is never returned.",
    responses(
        (status = 200, description = "API key list", body = Vec<Object>,
         example = json!([{
             "id": "550e8400-e29b-41d4-a716-446655440000",
             "key_prefix": "sk-gw-abc123",
             "name": "my-service",
             "scopes": ["chat"],
             "is_active": true
         }])),
        (status = 401, description = "Unauthorized", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn list_api_keys(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, serde_json::Value, bool)>(
        "SELECT id, key_prefix, name, scopes, is_active FROM api_keys ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| GatewayError::internal(e.to_string()))?;

    let keys: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, prefix, name, scopes, active)| {
            serde_json::json!({
                "id": id,
                "key_prefix": prefix,
                "name": name,
                "scopes": scopes,
                "is_active": active
            })
        })
        .collect();

    Ok(Json(serde_json::json!(keys)))
}

/// Revoke an API key (sets `is_active = false` and invalidates the Redis cache).
#[utoipa::path(
    delete,
    path = "/admin/keys/{id}",
    tag = "API Keys",
    summary = "Delete API key",
    description = "Revoke an API key. The Redis auth cache is invalidated immediately.",
    params(("id" = Uuid, Path, description = "API key UUID")),
    responses(
        (status = 204, description = "API key revoked"),
        (status = 401, description = "Unauthorized", body = GatewayError),
        (status = 404, description = "API key not found", body = GatewayError),
    ),
    security(("bearer" = []))
)]
pub async fn delete_api_key(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    // Get key_hash before deactivating (for cache invalidation)
    let key_hash: Option<String> =
        sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| GatewayError::internal(e.to_string()))?;

    let result = sqlx::query("UPDATE api_keys SET is_active = false WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| GatewayError::internal(e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(GatewayError::not_found("api key not found"));
    }

    // Invalidate Redis cache immediately (don't wait for TTL)
    if let Some(hash) = key_hash {
        crate::middleware::auth::invalidate_key_cache(&state, &hash).await;
    }

    Ok(StatusCode::NO_CONTENT)
}
