# Сравнение стратегий балансировки

## Описание стратегий

### Round-Robin

Циклический перебор: `counter % backends.len()`. Per-model `AtomicUsize`, lock-free.

**Плюсы:** простота, предсказуемость, нулевой overhead.
**Минусы:** не учитывает разницу в латентности или нагрузке провайдеров.
**Когда:** все реплики одинаковы по мощности.

---

### Weighted Round-Robin

Cumulative weights — провайдер с weight=3 получает 3 из 5 запросов при weights [3,1,1].

**Плюсы:** учитывает разную мощность, предсказуемое распределение.
**Минусы:** статичен, не адаптируется к runtime.
**Когда:** известно соотношение мощностей заранее.

Подтверждено тестами: `test_weighted_distribution` — weights [3,1] → 75%/25% (100 запросов: a=75, b=25).

---

### Latency-Based

Exponential Moving Average (EMA) per provider в Redis. Decay factor 0.3 — реагирует за ~5 запросов.

```
new_ema = 0.3 × current_latency_ms + 0.7 × old_ema
```

Sliding window 5 минут — фильтрует устаревшие данные. Выбирает провайдера с минимальным EMA.

**Плюсы:** адаптируется к текущей нагрузке, автоматически выбирает самого быстрого.
**Минусы:** требует Redis, задержка адаптации ~5 запросов, дополнительный round-trip на запись.
**Когда:** провайдеры с переменной латентностью (реальные API типа OpenAI варьируются 200ms–5s).

---

### Least-Connections

`AtomicUsize` per provider — инкремент в начале запроса, декремент по завершении.
Выбирает провайдера с минимумом активных in-flight запросов.

**Плюсы:** учитывает длительность запросов, равномерная загруженность.
**Минусы:** не учитывает скорость провайдера, только количество concurrent запросов.
**Когда:** запросы разной длительности (короткие completions vs длинные reasoning chains).

---

### Health-Aware

Round-robin с фильтрацией — unhealthy провайдеры исключаются из пула через circuit breaker.
Если все провайдеры unhealthy, допускает half-open для recovery.

**Плюсы:** автоматический обход нездоровых провайдеров, встроенный failover.
**Минусы:** проверка состояния circuit breaker на каждый запрос.
**Когда:** ненадёжные провайдеры, требование высокой доступности.

---

## Результаты нагрузочного тестирования

Условия: 50 VUs, 8s duration, 2s ramp-up, модель `mock-gpt` (3 реплики на localhost),
release build, host network mode.

| Стратегия | RPS | p50 | p95 | p99 | Error Rate | Overhead |
|-----------|-----|-----|-----|-----|-----------|---------|
| round-robin | **29 787** | **1.4ms** | **2.2ms** | 3.3ms | 0.00% | ~0 |
| weighted | 28 725 | 1.5ms | 2.3ms | 3.6ms | 0.00% | ~0 |
| least-connections | 25 282 | 1.7ms | 2.7ms | 4.2ms | 0.00% | AtomicUsize |
| health-aware | 24 666 | 1.7ms | 2.9ms | 4.6ms | 0.00% | CB check |
| latency | 20 665 | 2.1ms | 3.1ms | **4.6ms** | 0.00% | Redis EMA |

### Анализ overhead

- **round-robin**: минимальный overhead — один `AtomicUsize::fetch_add` (< 10ns)
- **weighted**: cumulative weight lookup по вектору — O(backends.len())
- **least-connections**: атомарный счётчик per provider + сравнение — незначительно медленнее
- **health-aware**: чтение состояния circuit breaker через `RwLock<HashMap>` — ~100ns
- **latency**: Redis async call на каждый запрос — +0.3–0.5ms на всех запросах

На mock-провайдерах (latency 50–200ms) разница в стратегиях незначительна.
На реальных LLM API (latency 0.5–5s) latency-based стратегия даёт существенный выигрыш.

---

## Circuit Breaker + Failover

Все стратегии фильтруют провайдеров через circuit breaker.
Health-aware применяет фильтрацию явно, остальные — через `is_available()` в `resolve()`.

Из нагрузочного теста (3 реплики, round-robin):

| Событие | Error Rate | p50 | Поведение |
|---------|-----------|-----|----------|
| 3/3 реплики работают | 0.00% | 1.0ms | Штатный режим |
| mock-1 убит (t=4s) | **0.00%** | 1.0ms | CB открылся за 5 failures, трафик на 2 реплики |
| mock-2 убит (t=9s) | **0.00%** | 1.0ms | CB открылся, весь трафик на 1 реплику |

0% ошибок при убийстве 2 из 3 провайдеров — circuit breaker прозрачно перенаправляет трафик.

State machine circuit breaker:

```
Closed ──(5 failures)──► Open ──(30s cooldown)──► HalfOpen ──(3 success)──► Closed
                                                       │
                                                  (1 failure)
                                                       │
                                                       ▼
                                                     Open
```

---

## Рекомендации для production LLM gateway

| Сценарий | Рекомендуемая стратегия | Обоснование |
|----------|------------------------|-------------|
| Мок-реплики с одинаковой латентностью | `round-robin` | Максимальный throughput, нулевой overhead |
| Несколько провайдеров разной мощности | `weighted` | Предсказуемое распределение по capacity |
| Мульти-провайдер (OpenAI + Anthropic) | `latency` | Автоматический выбор быстрейшего |
| Длинные reasoning/generation запросы | `least-connections` | Равномерная загруженность по времени |
| Ненадёжные провайдеры | `health-aware` | Явный failover, минимальный latency spike |

**Default** для большинства случаев: `round-robin` + circuit breaker. Этот режим покрывает 80%
сценариев с минимальным overhead и автоматическим failover.
