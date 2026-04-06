pub mod health;
pub mod latency;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::RoutingStrategy;
use crate::providers::LlmProvider;

use self::health::HealthTracker;
use self::latency::LatencyTracker;

struct Backend {
    provider_idx: usize,
    weight: u32,
}

struct ModelBackends {
    backends: Vec<Backend>,
    counter: AtomicUsize,
}

/// Per-provider cost rates, pre-computed at router build time.
#[derive(Clone, Copy, Default)]
pub struct CostRate {
    pub input: f64,
    pub output: f64,
}

pub struct Router {
    model_map: HashMap<String, ModelBackends>,
    providers: Vec<Box<dyn LlmProvider>>,
    in_flight: Vec<AtomicUsize>,
    /// provider_name → cost per token (avoids HashMap lookup — indexed by provider_idx)
    cost_rates: Vec<CostRate>,
    strategy: RoutingStrategy,
    pub health: HealthTracker,
    pub latency: Option<LatencyTracker>,
}

impl Router {
    pub fn new(
        providers: Vec<Box<dyn LlmProvider>>,
        weights: &HashMap<String, u32>,
        costs: &HashMap<String, CostRate>,
        strategy: RoutingStrategy,
        health: HealthTracker,
        latency: Option<LatencyTracker>,
    ) -> Self {
        let mut model_map: HashMap<String, Vec<Backend>> = HashMap::new();

        for (idx, provider) in providers.iter().enumerate() {
            let weight = weights.get(provider.name()).copied().unwrap_or(1);
            for model in provider.models() {
                model_map.entry(model.clone()).or_default().push(Backend {
                    provider_idx: idx,
                    weight,
                });
            }
        }

        let model_map = model_map
            .into_iter()
            .map(|(model, backends)| {
                (
                    model,
                    ModelBackends {
                        backends,
                        counter: AtomicUsize::new(0),
                    },
                )
            })
            .collect();

        let in_flight = (0..providers.len()).map(|_| AtomicUsize::new(0)).collect();

        // Index cost rates by provider position — O(1) lookup at request time
        let cost_rates: Vec<CostRate> = providers
            .iter()
            .map(|p| costs.get(p.name()).copied().unwrap_or_default())
            .collect();

        Self {
            model_map,
            in_flight,
            cost_rates,
            providers,
            strategy,
            health,
            latency,
        }
    }

    /// Resolve a provider for the given model.
    /// Filters out unhealthy providers, then applies routing strategy.
    pub async fn resolve(&self, model: &str) -> Option<&dyn LlmProvider> {
        let entry = self.model_map.get(model)?;
        if entry.backends.is_empty() {
            return None;
        }

        // Filter to healthy backends only
        let healthy: Vec<&Backend> = entry
            .backends
            .iter()
            .filter(|b| {
                let name = self.providers[b.provider_idx].name();
                self.health.is_available(name)
            })
            .collect();

        // If all are down, try all (circuit breaker might transition to half-open)
        let candidates = if healthy.is_empty() {
            entry.backends.iter().collect::<Vec<_>>()
        } else {
            healthy
        };

        let idx = match self.strategy {
            RoutingStrategy::RoundRobin => self.round_robin(entry, &candidates),
            RoutingStrategy::Weighted => self.weighted(entry, &candidates),
            RoutingStrategy::Latency => self.latency_based(&candidates).await,
            RoutingStrategy::LeastConnections => self.least_connections(&candidates),
            RoutingStrategy::HealthAware => self.round_robin(entry, &candidates),
        };

        Some(&*self.providers[idx])
    }

    /// Get a failover provider: different from `exclude`, healthy, same model.
    pub fn failover(&self, model: &str, exclude: &str) -> Option<&dyn LlmProvider> {
        let entry = self.model_map.get(model)?;

        entry
            .backends
            .iter()
            .filter(|b| {
                let p = &self.providers[b.provider_idx];
                p.name() != exclude && self.health.is_available(p.name())
            })
            .map(|b| &*self.providers[b.provider_idx])
            .next()
    }

    fn round_robin(&self, entry: &ModelBackends, candidates: &[&Backend]) -> usize {
        let counter = entry.counter.fetch_add(1, Ordering::Relaxed);
        candidates[counter % candidates.len()].provider_idx
    }

    fn weighted(&self, entry: &ModelBackends, candidates: &[&Backend]) -> usize {
        let total: usize = candidates.iter().map(|b| b.weight as usize).sum();
        if total == 0 {
            return candidates[0].provider_idx;
        }

        let counter = entry.counter.fetch_add(1, Ordering::Relaxed);
        let slot = counter % total;

        let mut cumulative = 0usize;
        for backend in candidates {
            cumulative += backend.weight as usize;
            if slot < cumulative {
                return backend.provider_idx;
            }
        }

        candidates.last().unwrap().provider_idx
    }

    /// Route to the provider with fewest in-flight requests.
    fn least_connections(&self, candidates: &[&Backend]) -> usize {
        candidates
            .iter()
            .min_by_key(|b| self.in_flight[b.provider_idx].load(Ordering::Relaxed))
            .unwrap()
            .provider_idx
    }

    /// Acquire an in-flight slot for a provider. Call release() when done.
    pub fn acquire(&self, provider_idx: usize) {
        if provider_idx < self.in_flight.len() {
            self.in_flight[provider_idx].fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn release(&self, provider_idx: usize) {
        if provider_idx < self.in_flight.len() {
            let _ = self.in_flight[provider_idx].fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |current| current.checked_sub(1),
            );
        }
    }

    pub fn provider_index(&self, name: &str) -> Option<usize> {
        self.providers.iter().position(|p| p.name() == name)
    }

    /// O(1) cost lookup by provider index. No allocation.
    #[inline]
    pub fn cost_rate(&self, provider_idx: usize) -> CostRate {
        self.cost_rates
            .get(provider_idx)
            .copied()
            .unwrap_or_default()
    }

    /// Compute cost for a request given token counts.
    #[inline]
    pub fn compute_cost(&self, provider_idx: usize, input_tokens: u32, output_tokens: u32) -> f64 {
        let rate = self.cost_rate(provider_idx);
        f64::from(input_tokens) * rate.input + f64::from(output_tokens) * rate.output
    }

    async fn latency_based(&self, candidates: &[&Backend]) -> usize {
        if let Some(tracker) = &self.latency {
            let names: Vec<&str> = candidates
                .iter()
                .map(|b| self.providers[b.provider_idx].name())
                .collect();

            if let Some(fastest) = tracker.fastest(&names).await {
                for b in candidates {
                    if self.providers[b.provider_idx].name() == fastest {
                        return b.provider_idx;
                    }
                }
            }
        }

        // Fallback to first candidate
        candidates[0].provider_idx
    }

    pub fn available_models(&self) -> Vec<&str> {
        let mut models: Vec<&str> = self.model_map.keys().map(|s| s.as_str()).collect();
        models.sort_unstable();
        models
    }

    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CircuitBreakerConfig, RoutingStrategy};

    struct FakeProvider {
        name: String,
        models: Vec<String>,
    }

    impl LlmProvider for FakeProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn models(&self) -> &[String] {
            &self.models
        }
        fn chat_completion<'a>(
            &'a self,
            _request: &'a crate::types::ChatRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            crate::types::ChatResponse,
                            crate::providers::ProviderError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            unimplemented!()
        }
        fn chat_completion_stream<'a>(
            &'a self,
            _request: &'a crate::types::ChatRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<reqwest::Response, crate::providers::ProviderError>,
                    > + Send
                    + 'a,
            >,
        > {
            unimplemented!()
        }
    }

    fn make_health() -> HealthTracker {
        HealthTracker::new(CircuitBreakerConfig::default())
    }

    fn make_providers() -> Vec<Box<dyn LlmProvider>> {
        vec![
            Box::new(FakeProvider {
                name: "a".into(),
                models: vec!["gpt".into()],
            }),
            Box::new(FakeProvider {
                name: "b".into(),
                models: vec!["gpt".into()],
            }),
        ]
    }

    #[tokio::test]
    async fn test_round_robin() {
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );
        let first = router.resolve("gpt").await.unwrap().name();
        let second = router.resolve("gpt").await.unwrap().name();
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn test_weighted_distribution() {
        let mut weights = HashMap::new();
        weights.insert("a".to_string(), 3);
        weights.insert("b".to_string(), 1);

        let router = Router::new(
            make_providers(),
            &weights,
            &HashMap::new(),
            RoutingStrategy::Weighted,
            make_health(),
            None,
        );

        let mut counts = HashMap::new();
        for _ in 0..100 {
            let name = router.resolve("gpt").await.unwrap().name().to_string();
            *counts.entry(name).or_insert(0) += 1;
        }

        assert_eq!(counts["a"], 75);
        assert_eq!(counts["b"], 25);
    }

    #[tokio::test]
    async fn test_available_models_sorted() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![Box::new(FakeProvider {
            name: "x".into(),
            models: vec!["z-model".into(), "a-model".into()],
        })];
        let router = Router::new(
            providers,
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );
        assert_eq!(router.available_models(), vec!["a-model", "z-model"]);
    }

    #[tokio::test]
    async fn test_unknown_model_returns_none() {
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );
        assert!(router.resolve("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_circuit_breaker_skips_unhealthy() {
        let health = make_health();
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            health.clone(),
            None,
        );

        // Trip circuit for provider "a"
        for _ in 0..5 {
            health.record_failure("a");
        }

        // All requests should go to "b"
        for _ in 0..10 {
            let name = router.resolve("gpt").await.unwrap().name();
            assert_eq!(name, "b");
        }
    }

    #[tokio::test]
    async fn test_failover_excludes_provider() {
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );
        let failover = router.failover("gpt", "a").unwrap();
        assert_eq!(failover.name(), "b");
    }

    #[test]
    fn test_cost_calculation() {
        let mut costs = HashMap::new();
        costs.insert(
            "a".to_string(),
            CostRate {
                input: 0.001,
                output: 0.002,
            },
        );

        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &costs,
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );

        let idx = router.provider_index("a").unwrap();
        let cost = router.compute_cost(idx, 100, 50);
        // 100 * 0.001 + 50 * 0.002 = 0.1 + 0.1 = 0.2
        assert!((cost - 0.2).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_least_connections_prefers_idle() {
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::LeastConnections,
            make_health(),
            None,
        );

        // Simulate 5 in-flight requests on provider "a" (index 0)
        let idx_a = router.provider_index("a").unwrap();
        for _ in 0..5 {
            router.acquire(idx_a);
        }

        // Least-connections should always pick "b" (0 in-flight)
        for _ in 0..10 {
            let name = router.resolve("gpt").await.unwrap().name();
            assert_eq!(name, "b");
        }

        // Release all and acquire on "b" instead
        for _ in 0..5 {
            router.release(idx_a);
        }
        let idx_b = router.provider_index("b").unwrap();
        router.acquire(idx_b);

        // Now "a" has 0 in-flight, "b" has 1 — should pick "a"
        let name = router.resolve("gpt").await.unwrap().name();
        assert_eq!(name, "a");
    }

    #[tokio::test]
    async fn test_per_model_counter_independence() {
        // Create providers where "a" serves "gpt" and "claude",
        // "b" serves only "gpt"
        let providers: Vec<Box<dyn LlmProvider>> = vec![
            Box::new(FakeProvider {
                name: "a".into(),
                models: vec!["gpt".into(), "claude".into()],
            }),
            Box::new(FakeProvider {
                name: "b".into(),
                models: vec!["gpt".into()],
            }),
        ];

        let router = Router::new(
            providers,
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );

        // Advance the "gpt" model counter several times
        for _ in 0..5 {
            router.resolve("gpt").await;
        }

        // The "claude" model counter should still be at 0,
        // so first resolve should return "a" (the only provider)
        let claude_provider = router.resolve("claude").await.unwrap().name();
        assert_eq!(claude_provider, "a");

        // Resolve "gpt" and "claude" again to confirm independence
        let gpt_name = router.resolve("gpt").await.unwrap().name();
        let claude_name = router.resolve("claude").await.unwrap().name();
        // claude only has one provider, always "a"
        assert_eq!(claude_name, "a");
        // gpt alternates; after 5 prior resolves (counter=5), next is index 5%2=1 => "b"
        assert_eq!(gpt_name, "b");
    }

    #[test]
    fn test_cost_default_zero() {
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
            make_health(),
            None,
        );

        let idx = router.provider_index("a").unwrap();
        assert_eq!(router.compute_cost(idx, 100, 50), 0.0);
    }
}
