# API

Все endpoints возвращают JSON. Ошибки — в формате `{"error": {"message": "...", "type": "..."}}`.

Интерактивная документация: **http://localhost:8080/scalar** (OpenAPI UI).

---

## LLM Proxy

Требуют `Authorization: Bearer sk-gw-...` со scope `chat`.

### POST /v1/chat/completions

OpenAI-совместимый endpoint.

**Запрос:**
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

**Ответ (stream=false):**
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

**Ответ (stream=true) — SSE:**
```
data: {"id":"...","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"}}]}

data: {"id":"...","choices":[{"delta":{"content":" world"}}]}

data: {"id":"...","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{...}}

data: [DONE]
```

### POST /v1/responses

OpenAI Responses API. Принимает `input` как строку или массив сообщений.

```json
{
  "model": "gpt-4o",
  "input": "Hello!"
}
```

### GET /v1/models

Возвращает список всех доступных моделей в формате OpenAI.

```json
{
  "object": "list",
  "data": [
    {"id": "mock-fast", "object": "model", "owned_by": "mock"},
    {"id": "mock-gpt",  "object": "model", "owned_by": "mock"}
  ]
}
```

### POST /v1/embeddings

```json
{"model": "text-embedding-3-small", "input": "Hello world"}
```

**Коды ошибок:**

| Код | Причина |
|-----|---------|
| 400 | Невалидный JSON, неизвестная модель, guardrail violation |
| 401 | Отсутствует или невалидный API-ключ |
| 403 | Недостаточный scope |
| 429 | Rate limit exceeded |
| 502 | Ошибка провайдера |
| 504 | TTFT timeout при стриминге |

---

## Provider Registry

Требуют scope `admin`.

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
  "weight": 3,
  "priority": 1
}
```

`provider_type`: `openai` · `openai-responses` · `anthropic` · `gemini` · `mock`

После создания routing table перестраивается автоматически — новая модель доступна без перезапуска.

### GET /admin/providers

Возвращает список активных провайдеров. API-ключи не возвращаются.

### GET /admin/providers/{id}

### PUT /admin/providers/{id}

Partial update — передавайте только изменяемые поля.

### DELETE /admin/providers/{id}

Soft delete (`is_active = false`). Провайдер немедленно исчезает из routing.

---

## A2A Agent Registry

Требуют scope `admin`. Discovery endpoint (`/.well-known/`) открыт без auth.

### POST /admin/agents

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

Минимум один skill обязателен.

### GET /admin/agents

### GET /admin/agents/{id}

### GET /admin/agents/{id}/.well-known/agent-card.json

A2A discovery endpoint — возвращает полную Agent Card. Без аутентификации.

### PUT /admin/agents/{id}

### DELETE /admin/agents/{id}

---

## API Keys

Требуют scope `admin`.

### POST /admin/keys

```json
{"name": "my-agent", "scopes": ["chat"], "rate_limit_rpm": 60}
```

**Ответ:**
```json
{
  "key": "sk-gw-abc123def456...",
  "name": "my-agent",
  "scopes": ["chat"],
  "warning": "save this key — it will not be shown again"
}
```

Ключ показывается **один раз**. В БД хранится только `sha256(key)`.

| Scope | Доступ |
|-------|--------|
| `chat` | `/v1/*` |
| `admin` | `/admin/*` |

### GET /admin/keys

Список ключей без сырого значения.

### DELETE /admin/keys/{id}

---

## Health

Без аутентификации.

### GET /health

```json
{"status": "healthy", "postgres": "ok", "redis": "ok", "uptime_secs": 120}
```

### GET /health/providers

Circuit breaker состояние каждого провайдера.

```json
{
  "mock-replica-1": "healthy",
  "mock-replica-2": "healthy",
  "mock-replica-3": "circuit_open",
  "openai": "half_open"
}
```
