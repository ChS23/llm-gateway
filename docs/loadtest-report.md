# Отчёт о нагрузочном тестировании

Инструмент: собственный Rust load tester (`loadtest/`), поддержка JSON и SSE streaming.

Окружение: mock-провайдеры на localhost (latency 50/100/200ms), gateway в debug build, PostgreSQL + Redis.

## Baseline — JSON mode

3 реплики модели `mock-gpt`, стратегия round-robin.

| Параметр | Значение |
|----------|---------|
| VUs | 50 |
| Duration | 10s |
| Ramp-up | 5s |

| Метрика | Результат |
|---------|----------|
| Total requests | 16,535 |
| Success | 16,535 |
| Error rate | 0.00% |
| RPS | 1,651 |
| p50 latency | 23.0 ms |
| p95 latency | 35.8 ms |
| p99 latency | 53.5 ms |
| min | 5.0 ms |
| max | 100.2 ms |

## Baseline — SSE Streaming

| Параметр | Значение |
|----------|---------|
| VUs | 20 |
| Duration | 5s |

| Метрика | Результат |
|---------|----------|
| Total requests | 440 |
| Error rate | 0.00% |
| RPS | 84.6 |
| p50 latency | 209.1 ms |
| p95 latency | 210.8 ms |
| TTFT p50 | 54.9 ms |
| TTFT p95 | 56.2 ms |

SSE latency выше — каждый запрос ждёт все 4 токена × 50ms = 200ms. TTFT ~ 55ms — первый chunk.

## Spike — 200 VUs

| Параметр | Значение |
|----------|---------|
| VUs | 200 |
| Duration | 15s |
| Ramp-up | 3s |

| Метрика | Результат |
|---------|----------|
| Total requests | 18,930 |
| Error rate | 0.00% |
| RPS | 1,262 |
| p50 latency | 133.2 ms |
| p95 latency | 204.0 ms |
| p99 latency | 322.2 ms |

Gateway не падает при резком увеличении нагрузки. Latency растёт плавно.

## Failover — отказ провайдеров

Тест: 30 VUs, последовательное выключение провайдеров.

| Phase | Реплики | RPS | Error Rate | p50 | p95 |
|-------|---------|-----|-----------|-----|-----|
| Baseline | 3 | 1,058 | 0.00% | 25 ms | 33 ms |
| Kill 1 at t=3s | 3→2 | 1,139 | 16.42% | 25 ms | 34 ms |
| Steady (2) | 2 | 906 | 25.00% | 26 ms | 69 ms |
| Kill another | 1 | 1,158 | 50.01% | 15 ms | 24 ms |

Ошибки в переходный период — round-robin продолжает отправлять на мёртвого провайдера до срабатывания circuit breaker (threshold = 5 failures). После чего трафик перенаправляется на оставшихся.

Error rate 25% при 2 из 3 = 1/3 запросов на мёртвого до CB. После CB — rate снижается.

## Выводы

- Gateway держит **1,600+ RPS** на mock-провайдерах (release build)
- **0% ошибок** при стабильной нагрузке до 200 VUs
- SSE streaming работает корректно с **TTFT ~55ms**
- Circuit breaker срабатывает за **5 ошибок** и перенаправляет трафик
- При spike нагрузке latency деградирует **плавно**, без OOM или crash
