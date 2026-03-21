use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::providers::LlmProvider;

pub struct Router {
    // model name → список провайдеров, которые эту модель поддерживают
    model_backends: HashMap<String, Vec<usize>>,
    // все провайдеры, индексируемые по позиции
    providers: Vec<Box<dyn LlmProvider>>,
    // глобальный счётчик для round-robin
    // AtomicUsize — lock-free, один fetch_add на запрос, минимальный contention
    counter: AtomicUsize,
}

impl Router {
    pub fn new(providers: Vec<Box<dyn LlmProvider>>) -> Self {
        let mut model_backends: HashMap<String, Vec<usize>> = HashMap::new();

        for (idx, provider) in providers.iter().enumerate() {
            for model in provider.models() {
                model_backends.entry(model.clone()).or_default().push(idx);
            }
        }

        Self {
            model_backends,
            providers,
            counter: AtomicUsize::new(0),
        }
    }

    pub fn resolve(&self, model: &str) -> Option<&dyn LlmProvider> {
        let backends = self.model_backends.get(model)?;
        if backends.is_empty() {
            return None;
        }

        // Relaxed — нам не нужен ordering с другими переменными,
        // достаточно атомарности самого инкремента.
        // Worst case: два потока получат одного провайдера — не страшно.
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        let backend_idx = backends[idx % backends.len()];
        Some(&*self.providers[backend_idx])
    }

    pub fn available_models(&self) -> Vec<&str> {
        self.model_backends.keys().map(|s| s.as_str()).collect()
    }
}
