# API

Все endpoints возвращают JSON. Ошибки — в формате `{"error": {"message": "...", "type": "..."}}`.

## LLM Proxy

### POST /v1/chat/completions

OpenAI-совместимый endpoint. Требует `Authorization: Bearer sk-gw-...`.

**Request:**
```json
{
  "model": "mock-fast",
  "messages": [
    {"role": "system", "content": "You are helpful"},
    {"role": "user", "content": "Hello"}
  ],
  "stream": false,
  "temperature": 0.7,
  "max_tokens": 100
}
```

**Response (JSON, stream=false):**
```json
{
  "id": "mock-mock:9001-1",
  "object": "chat.completion",
  "model": "mock-fast",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Hello from mock:9001!"},
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
}
```

**Response (SSE, stream=true):**
```
data: {"id":"...","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"}}]}

data: {"id":"...","choices":[{"delta":{"content":" world"}}]}

data: {"id":"...","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{...}}

data: [DONE]
```

**Ошибки:**
- `400` — невалидный JSON, неизвестная модель, guardrail violation
- `401` — отсутствует или невалидный API-ключ
- `403` — недостаточный scope
- `429` — rate limit exceeded
- `502` — ошибка провайдера
- `504` — TTFT timeout

---

## Provider Registry

Admin endpoints — требуют `Authorization: Bearer sk-gw-...` с scope `admin`.

### POST /admin/providers

```json
{
  "name": "my-openai",
  "provider_type": "openai",
  "base_url": "https://api.openai.com/v1",
  "api_key": "sk-...",
  "models": ["gpt-4o", "gpt-4o-mini"],
  "cost_per_input_token": 0.0000025,
  "cost_per_output_token": 0.00001,
  "weight": 3
}
```

Поддерживаемые `provider_type`: `openai`, `openai-responses`, `anthropic`, `gemini`, `mock`.

После создания — routing table перестраивается автоматически (hot reload).

### GET /admin/providers
### GET /admin/providers/{id}
### PUT /admin/providers/{id}

Partial update через COALESCE — передавайте только изменяемые поля.

### DELETE /admin/providers/{id}

Soft delete (`is_active = false`). Провайдер исчезает из routing.

---

## A2A Agent Registry

### POST /admin/agents

Регистрация агента по спецификации A2A Protocol v1.0.

```json
{
  "name": "Code Review Agent",
  "description": "Automated PR reviewer",
  "url": "https://agents.example.com/review/a2a",
  "version": "1.0.0",
  "skills": [
    {"id": "review_pr", "name": "Review PR", "description": "Analyzes code", "tags": ["github"]}
  ],
  "capabilities": {"streaming": true},
  "security": {"schemes": ["Bearer"]}
}
```

Валидация: минимум один skill обязателен.

### GET /admin/agents
### GET /admin/agents/{id}
### GET /admin/agents/{id}/.well-known/agent-card.json

A2A discovery endpoint — возвращает полную Agent Card как при регистрации.

### PUT /admin/agents/{id}
### DELETE /admin/agents/{id}

---

## API Keys

### POST /admin/keys

```json
{
  "name": "my-agent",
  "scopes": ["chat"],
  "rate_limit_rpm": 60
}
```

**Response:**
```json
{
  "key": "sk-gw-abc123def456...",
  "key_prefix": "sk-gw-abc123",
  "name": "my-agent",
  "scopes": ["chat"],
  "warning": "save this key — it will not be shown again"
}
```

Ключ показывается **один раз**. В БД хранится только `sha256(key)`.

Scopes:
- `chat` — доступ к `/v1/*`
- `admin` — доступ к `/admin/*`

### GET /admin/keys

Возвращает список ключей (prefix, name, scopes, is_active). Сам ключ не возвращается.

### DELETE /admin/keys/{id}

---

## Health

### GET /health

Без аутентификации. Возвращает JSON со статусом всех зависимостей.

```json
{"status": "healthy", "postgres": "ok", "redis": "ok", "uptime_secs": 120}
```

### GET /health/providers

Без аутентификации. Circuit breaker состояние каждого провайдера.

```json
{
  "mock-replica-1": "healthy",
  "mock-replica-2": "healthy",
  "mock-replica-3": "circuit_open",
  "openai": "half_open"
}
```
