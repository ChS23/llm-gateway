use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::RoutingStrategy;
use crate::providers::LlmProvider;

struct Backend {
    provider_idx: usize,
    weight: u32,
}

pub struct Router {
    model_backends: HashMap<String, Vec<Backend>>,
    providers: Vec<Box<dyn LlmProvider>>,
    counter: AtomicUsize,
    strategy: RoutingStrategy,
}

impl Router {
    pub fn new(
        providers: Vec<Box<dyn LlmProvider>>,
        weights: &HashMap<String, u32>,
        strategy: RoutingStrategy,
    ) -> Self {
        let mut model_backends: HashMap<String, Vec<Backend>> = HashMap::new();

        for (idx, provider) in providers.iter().enumerate() {
            let weight = weights.get(provider.name()).copied().unwrap_or(1);
            for model in provider.models() {
                model_backends
                    .entry(model.clone())
                    .or_default()
                    .push(Backend {
                        provider_idx: idx,
                        weight,
                    });
            }
        }

        Self {
            model_backends,
            providers,
            counter: AtomicUsize::new(0),
            strategy,
        }
    }

    pub fn resolve(&self, model: &str) -> Option<&dyn LlmProvider> {
        let backends = self.model_backends.get(model)?;
        if backends.is_empty() {
            return None;
        }

        let idx = match self.strategy {
            RoutingStrategy::RoundRobin => self.round_robin(backends),
            RoutingStrategy::Weighted => self.weighted(backends),
            // Latency и HealthAware будут в Phase 2 (нужен Redis)
            RoutingStrategy::Latency | RoutingStrategy::HealthAware => self.round_robin(backends),
        };

        Some(&*self.providers[idx])
    }

    fn round_robin(&self, backends: &[Backend]) -> usize {
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        backends[counter % backends.len()].provider_idx
    }

    /// Weighted round-robin через cumulative weights.
    /// Пример: weights [3, 1, 1] → cumulative [3, 4, 5]
    /// counter % 5: 0,1,2 → provider 0; 3 → provider 1; 4 → provider 2
    fn weighted(&self, backends: &[Backend]) -> usize {
        let total: u32 = backends.iter().map(|b| b.weight).sum();
        if total == 0 {
            return backends[0].provider_idx;
        }

        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let slot = (counter as u32) % total;

        let mut cumulative = 0;
        for backend in backends {
            cumulative += backend.weight;
            if slot < cumulative {
                return backend.provider_idx;
            }
        }

        backends.last().unwrap().provider_idx
    }

    pub fn available_models(&self) -> Vec<&str> {
        self.model_backends.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoutingStrategy;

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

    #[test]
    fn test_round_robin() {
        let router = Router::new(
            make_providers(),
            &HashMap::new(),
            RoutingStrategy::RoundRobin,
        );
        let first = router.resolve("gpt").unwrap().name();
        let second = router.resolve("gpt").unwrap().name();
        assert_ne!(first, second);
    }

    #[test]
    fn test_weighted_distribution() {
        let mut weights = HashMap::new();
        weights.insert("a".to_string(), 3);
        weights.insert("b".to_string(), 1);

        let router = Router::new(make_providers(), &weights, RoutingStrategy::Weighted);

        let mut counts = HashMap::new();
        for _ in 0..100 {
            let name = router.resolve("gpt").unwrap().name().to_string();
            *counts.entry(name).or_insert(0) += 1;
        }

        // 3:1 ratio → "a" should get 75, "b" should get 25
        assert_eq!(counts["a"], 75);
        assert_eq!(counts["b"], 25);
    }
}
