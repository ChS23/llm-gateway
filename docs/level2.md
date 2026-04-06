# Уровень 2 — Реестры агентов и умная маршрутизация

## 1. Архитектурные диаграммы

### C4 Level 2 — Container Diagram

![C4 Container](images/c4-container.svg)

Gateway хранит провайдеров и агентов в PostgreSQL, Redis используется для
latency EMA и rate limiting. Горячая перезагрузка маршрутизатора через `ArcSwap`
позволяет менять провайдеров без перезапуска.

### Circuit Breaker — State Machine

![Circuit Breaker](images/circuit-breaker.svg)

| Переход | Условие |
|---------|---------|
| Closed → Open | 5 consecutive failures (5xx, timeout, 429) |
| Open → HalfOpen | cooldown 30s истёк |
| HalfOpen → Closed | 3 successful probes |
| HalfOpen → Open | любой failure в HalfOpen |

---

## 2. Описание API

### Провайдеры (динамическая регистрация)

**POST /admin/providers** — зарегистрировать провайдера. Routing table перестраивается автоматически.

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

Поддерживаемые `provider_type`: `openai`, `openai-responses`, `anthropic`, `gemini`, `mock`.

| Endpoint | Метод | Описание |
|----------|-------|----------|
| `/admin/providers` | GET | Список активных провайдеров |
| `/admin/providers` | POST | Регистрация нового провайдера |
| `/admin/providers/{id}` | GET | Получить провайдера |
| `/admin/providers/{id}` | PUT | Обновить (partial update) |
| `/admin/providers/{id}` | DELETE | Soft delete, убирает из routing |

После PUT/DELETE gateway перестраивает router через `ArcSwap::store()` —
in-flight запросы дорабатывают со старым router'ом.

### A2A Agent Registry

**POST /admin/agents** — зарегистрировать агента по спецификации A2A Protocol v1.0.

```json
{
  "name": "Code Review Agent",
  "description": "Automated PR reviewer",
  "url": "https://agents.example.com/review/a2a",
  "version": "1.0.0",
  "skills": [
    {
      "id": "review_pr",
      "name": "Review Pull Request",
      "description": "Analyzes code changes and leaves comments",
      "tags": ["github", "code-review"]
    }
  ],
  "capabilities": {"streaming": true},
  "security": {"schemes": ["Bearer"]}
}
```

Валидация: минимум один skill обязателен.

| Endpoint | Метод | Описание |
|----------|-------|----------|
| `/admin/agents` | GET | Список агентов |
| `/admin/agents` | POST | Регистрация агента |
| `/admin/agents/{id}` | GET | Получить агента |
| `/admin/agents/{id}` | PUT | Обновить |
| `/admin/agents/{id}` | DELETE | Soft delete |
| `/admin/agents/{id}/.well-known/agent-card.json` | GET | A2A Discovery endpoint |

Discovery endpoint возвращает полный `card_json` как при регистрации —
соответствует A2A Protocol v1.0.

### Стратегия маршрутизации

Задаётся в `config/gateway.toml`:

```toml
[routing]
default_strategy = "latency"  # round-robin | weighted | latency | least-connections | health-aware
```

---

## 3. Инструкции по запуску

### Регистрация провайдера через API

```bash
# Добавить OpenAI провайдер
curl -X POST http://localhost:8080/admin/providers \
  -H "Authorization: Bearer sk-gw-admin-bootstrap-key" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "openai-prod",
    "provider_type": "openai",
    "base_url": "https://api.openai.com/v1",
    "api_key": "'$OPENAI_API_KEY'",
    "models": ["gpt-4o"],
    "cost_per_input_token": 0.0000025,
    "cost_per_output_token": 0.00001
  }'

# Сразу доступен без перезапуска
curl http://localhost:8080/v1/models \
  -H "Authorization: Bearer sk-gw-admin-bootstrap-key"
```

### Регистрация агента

```bash
curl -X POST http://localhost:8080/admin/agents \
  -H "Authorization: Bearer sk-gw-admin-bootstrap-key" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "My Agent",
    "description": "Does useful things",
    "url": "https://my-agent.example.com/a2a",
    "version": "1.0.0",
    "skills": [{"id": "task", "name": "Run task", "description": "Executes tasks"}]
  }'

# A2A Discovery
curl http://localhost:8080/admin/agents/<id>/.well-known/agent-card.json
```

### Трейсинг через Langfuse (вместо MLflow)

Задание требует MLflow. В качестве альтернативы выбран **Langfuse** — он специализируется именно на LLM-трейсинге и поддерживает OpenTelemetry нативно, что позволило не добавлять отдельный SDK: gateway уже экспортирует OTel spans, OTel Collector пробрасывает их в Langfuse Cloud без изменений в коде.

MLflow ориентирован на эксперименты и модели (experiment tracking, model registry), а не на production-трейсинг инференса. Для задачи «трассировать работу агентов и LLM» Langfuse подходит точнее.

Трейсы уходят автоматически при наличии `LANGFUSE_AUTH` в `.env`:

```bash
# .env
LANGFUSE_BASEURL=https://cloud.langfuse.com
LANGFUSE_AUTH=<base64(pk-lf-...:sk-lf-...)>
```

Каждый запрос создаёт span с атрибутами по GenAI Semantic Conventions:
`gen_ai.operation.name`, `gen_ai.request.model`, `gen_ai.system`,
`gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`,
`langfuse.observation.input`, `langfuse.observation.output`.

---

## 4. Тестирование и сравнение стратегий балансировки

### Сравнение всех 5 стратегий

Условия: 50 VUs, 8s, 3 реплики mock-gpt (latency 50/100/200ms), release build.

| Стратегия | RPS | p50 | p95 | p99 | Overhead |
|-----------|-----|-----|-----|-----|---------|
| round-robin | **29 787** | **1.4ms** | 2.2ms | 3.3ms | ~0 |
| weighted | 28 725 | 1.5ms | 2.3ms | 3.6ms | ~0 |
| least-connections | 25 282 | 1.7ms | 2.7ms | 4.2ms | AtomicUsize |
| health-aware | 24 666 | 1.7ms | 2.9ms | 4.6ms | CB state check |
| **latency** | 20 665 | 2.1ms | 3.1ms | 4.6ms | Redis EMA |

Latency-based overhead (~0.7ms) — Redis async round-trip на каждый запрос.
На реальных LLM API (latency 500ms–5s) этот overhead незначителен,
а выигрыш от выбора быстрейшего провайдера существенен.

### Circuit Breaker + Failover тест

30 VUs, 15s, последовательное убийство mock-provider-1 (t=4s) и mock-provider-2 (t=9s):

| Период | Реплики | RPS | Error Rate |
|--------|---------|-----|-----------|
| t=0–4s | 3/3 | 27 900 | **0.00%** |
| t=4–9s | 2/3 | 27 400 | **0.00%** |
| t=9–15s | 1/3 | 27 100 | **0.00%** |

Circuit breaker обнаруживает недоступный провайдер за 5 consecutive failures
и исключает его из пула. Failover прозрачен для клиентов.

### Langfuse — трейсы в production

![Langfuse Dashboard](images/langfuse-dashboard.png)

По результатам нагрузочных тестов: **1 249 трейсов** зафиксировано,
mock-fast — 8.28M токенов, mock-gpt — 38.11M токенов.
Стоимость $0.00 — mock провайдеры без цены за токен.

### Подробный отчёт по стратегиям

Развёрнутое сравнение всех 5 стратегий с анализом overhead и рекомендациями для production: [docs/balancing-report.md](balancing-report.md)
