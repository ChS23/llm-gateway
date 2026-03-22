use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use uuid::Uuid;

use crate::models::agent::{Agent, CreateAgent, UpdateAgent};
use crate::models::provider::{CreateProvider, Provider, UpdateProvider};
use crate::state::SharedState;
use crate::types::GatewayError;

// -- Providers ----------------------------------------------------------------

pub async fn create_provider(
    State(state): State<SharedState>,
    Json(input): Json<CreateProvider>,
) -> Result<impl IntoResponse, GatewayError> {
    let models_json = serde_json::to_value(&input.models).unwrap();

    let provider = sqlx::query_as::<_, Provider>(
        r#"
        INSERT INTO providers (name, provider_type, base_url, models, cost_per_input_token,
                               cost_per_output_token, rate_limit_rpm, priority, weight)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING *
        "#,
    )
    .bind(&input.name)
    .bind(&input.provider_type)
    .bind(&input.base_url)
    .bind(&models_json)
    .bind(input.cost_per_input_token)
    .bind(input.cost_per_output_token)
    .bind(input.rate_limit_rpm)
    .bind(input.priority)
    .bind(input.weight)
    .fetch_one(&state.db)
    .await
    .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    Ok((StatusCode::CREATED, Json(provider)))
}

pub async fn list_providers(
    State(state): State<SharedState>,
) -> Result<Json<Vec<Provider>>, GatewayError> {
    let providers = sqlx::query_as::<_, Provider>(
        "SELECT * FROM providers WHERE is_active = true ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    Ok(Json(providers))
}

pub async fn get_provider(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Provider>, GatewayError> {
    let provider = sqlx::query_as::<_, Provider>("SELECT * FROM providers WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?
        .ok_or_else(|| GatewayError::not_found("provider not found"))?;

    Ok(Json(provider))
}

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
    .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?
    .ok_or_else(|| GatewayError::not_found("provider not found"))?;

    Ok(Json(provider))
}

pub async fn delete_provider(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    let result =
        sqlx::query("UPDATE providers SET is_active = false, updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(GatewayError::not_found("provider not found"));
    }

    Ok(StatusCode::NO_CONTENT)
}

// -- Agents -------------------------------------------------------------------

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
    .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    Ok((StatusCode::CREATED, Json(agent)))
}

pub async fn list_agents(
    State(state): State<SharedState>,
) -> Result<Json<Vec<Agent>>, GatewayError> {
    let agents =
        sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE is_active = true ORDER BY name")
            .fetch_all(&state.db)
            .await
            .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    Ok(Json(agents))
}

pub async fn get_agent(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Agent>, GatewayError> {
    let agent = sqlx::query_as::<_, Agent>("SELECT * FROM agents WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?
        .ok_or_else(|| GatewayError::not_found("agent not found"))?;

    Ok(Json(agent))
}

/// A2A discovery endpoint — returns the agent card JSON as-is.
pub async fn get_agent_card(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let card: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT card_json FROM agents WHERE id = $1 AND is_active = true")
            .bind(id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    let (card_json,) = card.ok_or_else(|| GatewayError::not_found("agent not found"))?;
    Ok(Json(card_json))
}

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
    .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?
    .ok_or_else(|| GatewayError::not_found("agent not found"))?;

    Ok(Json(agent))
}

pub async fn delete_agent(
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, GatewayError> {
    let result =
        sqlx::query("UPDATE agents SET is_active = false, updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&state.db)
            .await
            .map_err(|e| GatewayError::bad_request("db_error", e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(GatewayError::not_found("agent not found"));
    }

    Ok(StatusCode::NO_CONTENT)
}
