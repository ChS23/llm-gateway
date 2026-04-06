# Уровень 1 — Gateway, балансировщик, мониторинг

## 1. Архитектурные диаграммы

### C4 Level 1 — System Context

![C4 Context](images/c4-context.svg)

Система принимает OpenAI-совместимые запросы от разработчиков и агентов, проксирует
их к LLM-провайдерам, трейсы уходят в Langfuse.

### Поток запроса

![Поток запроса](images/request-flow.svg)

Middleware pipeline (Tower layers, порядок критичен):

```
Body Limit (1MB) → Auth → Guardrails → Router → Provider → Response
```

Для streaming: ответ провайдера проксируется chunk-by-chunk через SSE Proxy
без буферизации — соединение не разрывается.

---

## 2. Описание API

### POST /v1/chat/completions

OpenAI-совместимый endpoint. Требует `Authorization: Bearer sk-gw-...`.

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

**Ответ (JSON, stream=false):**
```json
{
  "id": "mock-mock:9001-1",
  "object": "chat.completion",
  "model": "mock-fast",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Hello from mock!"},
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
}
```

**Ответ (SSE, stream=true):**
```
data: {"id":"...","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"}}]}

data: {"id":"...","choices":[{"delta":{"content":" world"}}]}

data: {"id":"...","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{...}}

data: [DONE]
```

### GET /health

```json
{"status": "healthy", "postgres": "ok", "redis": "ok", "uptime_secs": 120}
```

### GET /health/providers

```json
{
  "mock-replica-1": "healthy",
  "mock-replica-2": "healthy",
  "mock-replica-3": "healthy"
}
```

### GET /v1/models

Возвращает список доступных моделей в формате OpenAI.

**Коды ошибок:**

| Код | Причина |
|-----|---------|
| 400 | Невалидный JSON, неизвестная модель |
| 401 | Отсутствует или невалидный API-ключ |
| 429 | Rate limit exceeded |
| 502 | Ошибка провайдера |
| 504 | TTFT timeout при стриминге |

---

## 3. Инструкции по запуску

### Требования

- Docker + Docker Compose
- Linux (для host network mode)

### Запуск

```bash
git clone <repo> && cd llm-gateway
cp .env.example .env
docker compose up -d
```

`COMPOSE_FILE=compose.yml:compose.host.yml` уже прописан в `.env` —
все сервисы поднимаются в host network режиме.

### Сервисы после запуска

| Сервис | Адрес |
|--------|-------|
| Gateway | http://localhost:8080 |
| Grafana | http://localhost:3000 (admin/admin) |
| Prometheus | http://localhost:9090 |

### Первый запрос

```bash
# Создать API-ключ
curl -s -X POST http://localhost:8080/admin/keys \
  -H "Authorization: Bearer sk-gw-admin-bootstrap-key" \
  -H "Content-Type: application/json" \
  -d '{"name": "test", "scopes": ["chat"]}' | jq .key

# Запрос к mock провайдеру
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "hi"}]}'

# SSE streaming
curl -N http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "hi"}], "stream": true}'
```

### Конфигурация провайдеров (`config/gateway.toml`)

```toml
[routing]
default_strategy = "round-robin"  # round-robin | weighted | latency | least-connections | health-aware

[[providers]]
name = "mock-replica-1"
type = "mock"
base_url = "http://127.0.0.1:9001"
models = ["mock-gpt", "mock-fast"]
weight = 3

[[providers]]
name = "mock-replica-2"
type = "mock"
base_url = "http://127.0.0.1:9002"
models = ["mock-gpt"]
weight = 1
```

---

## 4. Тестирование и сравнение стратегий балансировки

### Инструмент

Собственный Rust load tester (`loadtest/`).

```bash
# Запуск нагрузочного теста
API_KEY=sk-gw-admin-bootstrap-key MODEL=mock-fast \
  CONCURRENCY=50 DURATION=10 \
  cargo run --release -p loadtest
```

### Baseline — JSON mode (50 VUs, 10s)

| Метрика | Результат |
|---------|----------|
| RPS | **28 603** |
| p50 latency | 1.4 ms |
| p95 latency | 2.3 ms |
| p99 latency | 3.8 ms |
| Error rate | **0.00%** |

### SSE Streaming (20 VUs, 10s)

| Метрика | Результат |
|---------|----------|
| RPS | 87.7 |
| p50 latency | 204.6 ms |
| TTFT p50 | **51.4 ms** |
| TTFT p95 | 52.1 ms |
| Error rate | **0.00%** |

SSE latency включает полную генерацию (4 токена × 50ms = 200ms).
TTFT — время до первого токена, 51ms.

### Сравнение round-robin и weighted

| Стратегия | RPS | p50 | p95 | Особенность |
|-----------|-----|-----|-----|-------------|
| round-robin | **29 787** | **1.4ms** | 2.2ms | Равномерное распределение |
| weighted [3,1,1] | 28 725 | 1.5ms | 2.3ms | replica-1 получает 60% трафика |

Weighted стратегия подтверждена unit-тестом: `test_weighted_distribution` — 
weights [3,1] → 75%/25% на 100 запросах.

### Grafana — наблюдаемость

Дашборд с 10 панелями: RPS, error rate, p95 TTFT, TTFT/TPOT distribution,
token usage, cost, provider health, latency heatmap, CPU/Memory.

![Grafana — RPS, Error Rate, TTFT](images/grafana-top.png)

![Grafana — Token Usage, Cost, Provider Health](images/grafana-middle.png)

![Grafana — CPU, Memory](images/grafana-bottom.png)

Метрики по GenAI Semantic Conventions (OpenTelemetry):
`llm_gateway.requests.total`, `gen_ai.client.operation.duration`,
`gen_ai.server.time_to_first_token`, `gen_ai.server.time_per_output_token`,
`gen_ai.client.token.usage`, `process.cpu.utilization`.
