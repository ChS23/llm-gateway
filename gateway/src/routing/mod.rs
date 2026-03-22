use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::RoutingStrategy;
use crate::providers::LlmProvider;

struct Backend {
    provider_idx: usize,
    weight: u32,
}

struct ModelBackends {
    backends: Vec<Backend>,
    counter: AtomicUsize,
}

pub struct Router {
    model_map: HashMap<String, ModelBackends>,
    providers: Vec<Box<dyn LlmProvider>>,
    strategy: RoutingStrategy,
}

impl Router {
    pub fn new(
        providers: Vec<Box<dyn LlmProvider>>,
        weights: &HashMap<String, u32>,
        strategy: RoutingStrategy,
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

        Self {
            model_map,
            providers,
            strategy,
        }
    }

    pub fn resolve(&self, model: &str) -> Option<&dyn LlmProvider> {
        let entry = self.model_map.get(model)?;
        if entry.backends.is_empty() {
            return None;
        }

        let idx = match self.strategy {
            RoutingStrategy::RoundRobin => Self::round_robin(entry),
            RoutingStrategy::Weighted => Self::weighted(entry),
            RoutingStrategy::Latency | RoutingStrategy::HealthAware => Self::round_robin(entry),
        };

        Some(&*self.providers[idx])
    }

    fn round_robin(entry: &ModelBackends) -> usize {
        let counter = entry.counter.fetch_add(1, Ordering::Relaxed);
        entry.backends[counter % entry.backends.len()].provider_idx
    }

    fn weighted(entry: &ModelBackends) -> usize {
        let total: usize = entry.backends.iter().map(|b| b.weight as usize).sum();
        if total == 0 {
            return entry.backends[0].provider_idx;
        }

        let counter = entry.counter.fetch_add(1, Ordering::Relaxed);
        let slot = counter % total;

        let mut cumulative = 0usize;
        for backend in &entry.backends {
            cumulative += backend.weight as usize;
            if slot < cumulative {
                return backend.provider_idx;
            }
        }

        entry.backends.last().unwrap().provider_idx
    }

    pub fn available_models(&self) -> Vec<&str> {
        let mut models: Vec<&str> = self.model_map.keys().map(|s| s.as_str()).collect();
        models.sort_unstable();
        models
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

        assert_eq!(counts["a"], 75);
        assert_eq!(counts["b"], 25);
    }

    #[test]
    fn test_available_models_sorted() {
        let providers: Vec<Box<dyn LlmProvider>> = vec![Box::new(FakeProvider {
            name: "x".into(),
            models: vec!["z-model".into(), "a-model".into()],
        })];
        let router = Router::new(providers, &HashMap::new(), RoutingStrategy::RoundRobin);
        assert_eq!(router.available_models(), vec!["a-model", "z-model"]);
    }
}
