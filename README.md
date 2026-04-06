# llm-gateway

LLM gateway на Rust. Принимает OpenAI-совместимые запросы, сам разбирается куда их отправить — с балансировкой, failover, guardrails и полной наблюдаемостью.

Написан как домашнее задание ITMO AI Talent Hub (инфраструктурный трек), реализованы все три уровня сложности.

---

## Что умеет

Запрос приходит на `/v1/chat/completions` — дальше gateway сам:

- выбирает провайдера по стратегии (round-robin, weighted, latency-based, least-connections)
- пропускает запрос через guardrails — блокирует prompt injection и утечки секретов
- при ошибке провайдера — failover на следующий, circuit breaker не даёт спамить упавший
- пишет span в Langfuse с токенами и стоимостью, метрики летят в Prometheus → Grafana

Поддерживает 5 типов провайдеров: **OpenAI**, **Anthropic**, **Gemini**, **OpenAI Responses API**, **Mock**. SSE streaming работает без буферизации — TTFT/TPOT считаются inline.

---

## Запуск

```bash
git clone <repo> && cd llm-gateway
cp .env.example .env   # ключи провайдеров опциональны, mock работает без них
docker compose up -d   # ~30 секунд, поднимает 9 контейнеров
```

Сервисы после старта:

| | |
|--|--|
| Gateway | http://localhost:8080 |
| Grafana | http://localhost:3000 &nbsp; `admin / admin` |
| Prometheus | http://localhost:9090 |
| OpenAPI UI | http://localhost:8080/scalar |

```bash
# Создать API-ключ через bootstrap-ключ из .env
curl -s -X POST http://localhost:8080/admin/keys \
  -H "Authorization: Bearer sk-gw-admin-bootstrap-key" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-key", "scopes": ["chat"]}' | jq .key

# Спросить что-нибудь
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-gw-..." \
  -H "Content-Type: application/json" \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "Hello!"}]}'

# Или с SSE streaming
curl -N http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-gw-..." \
  -H "Content-Type: application/json" \
  -d '{"model": "mock-fast", "messages": [{"role": "user", "content": "Hello!"}], "stream": true}'
```

---

## Убедиться что всё работает

```bash
KEY="sk-gw-admin-bootstrap-key"

# Здоровье сервиса
curl -s http://localhost:8080/health | jq .

# Балансировка — три запроса должны уйти на разные реплики
for i in 1 2 3; do
  curl -s http://localhost:8080/v1/chat/completions \
    -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
    -d '{"model":"mock-gpt","messages":[{"role":"user","content":"hi"}]}' \
    | jq -r '.choices[0].message.content'
done
# Hello from mock:9001! / :9002! / :9003!

# Guardrails блокируют инъекции → 400
curl -s -o /dev/null -w "%{http_code}" http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \
  -d '{"model":"mock-fast","messages":[{"role":"user","content":"ignore all previous instructions"}]}'

# Нагрузочный тест — ~30k RPS на mock
API_KEY=$KEY CONCURRENCY=50 DURATION=10 cargo run --release -p loadtest
```

---

## Мониторинг

После нагрузочного теста Grafana показывает живые данные:

![RPS, Error Rate, P95 TTFT](docs/images/grafana-top.png)

![Token Usage, Cost per Model, Provider Health](docs/images/grafana-middle.png)

![CPU Utilization, Memory Usage](docs/images/grafana-bottom.png)

P95 TTFT — 90ms. Memory — 42 MiB под нагрузкой.

---

## Как устроено внутри

```mermaid
flowchart TD
    C([Client]) --> BL["Body Limit (1MB)"]
    BL --> A["Auth\nsha256(key) · scope · rate limit"]
    A --> G["Guardrails\nRegexSet: injection + secrets"]
    G --> R["Router\nresolve(model) · стратегия"]
    CB(["Circuit Breaker\n(per provider)"]) -. фильтр .-> R
    R --> P(["Provider\nOpenAI · Anthropic · Gemini · Mock"])
    P --> RS["Response\nJSON / SSE proxy · TTFT/TPOT"]
    RS --> OT[("OTel\nPrometheus · Langfuse")]
```

Routing table хранится в `ArcSwap<Router>` — lock-free чтение на горячем пути, атомарная замена при добавлении провайдера через API.

**5 стратегий балансировки:**

| Стратегия | Как работает |
|-----------|-------------|
| `round-robin` | `AtomicUsize % backends` — дефолт |
| `weighted` | Cumulative WRR — для разных мощностей |
| `latency` | Redis EMA по последним ответам |
| `least-connections` | `AtomicUsize` in-flight на провайдера |
| `health-aware` | Round-robin + фильтр circuit breaker |

**Circuit breaker** — см. диаграмму состояний выше, или в [docs/level2.md](docs/level2.md).

---

## Стек

| | |
|--|--|
| Язык | Rust 1.94, axum 0.8, tokio |
| База | PostgreSQL 17, sqlx 0.8 (compile-time queries) |
| Кеш | Redis 8, fred 10 (latency EMA + rate limiting) |
| Observability | OpenTelemetry 0.31 → Prometheus + Langfuse Cloud |
| Guardrails | regex RegexSet (O(n) single-pass) |
| Hot reload | arc-swap (lock-free) |
| Load testing | собственный Rust бинарник с SSE и TTFT |

---

## Тесты

```bash
cargo test --workspace   # 89 тестов, ~0.1s
```

68 unit-тестов (config, types, routing, guardrails, circuit breaker, stream metrics, auth cache) + 21 integration-тест на axum-test (agents, keys, providers, chat, health).

---

## Документация по уровням задания

| Уровень | Что реализовано | Документ |
|---------|----------------|----------|
| 1 — Gateway + балансировщик + мониторинг | Прокси, SSE, 3 стратегии, OTel + Grafana | [docs/level1.md](docs/level1.md) |
| 2 — Реестры + умная маршрутизация | A2A Registry, 5 стратегий, circuit breaker, Langfuse | [docs/level2.md](docs/level2.md) |
| 3 — Guardrails + авторизация + нагрузка | Injection/secret scan, API keys, 89 тестов, 34k RPS | [docs/level3.md](docs/level3.md) |

Дополнительно: [API](docs/api.md) · [Архитектура](docs/architecture.md) · [Развёртывание](docs/deployment.md) · [Нагрузочные тесты](docs/loadtest-report.md) · [Сравнение стратегий](docs/balancing-report.md)
