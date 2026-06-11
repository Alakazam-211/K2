pub mod download;
pub mod file_index;
pub mod tools;

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

/// Process-wide singleton LlmManager. Same shape as
/// `crate::terminal::shared()` / `crate::db::shared()` — one handle,
/// one underlying instance, accessible from anywhere in k2so-core.
static SHARED: OnceLock<Arc<Mutex<LlmManager>>> = OnceLock::new();

/// Return (and lazy-initialize) the shared LlmManager. AppState holds
/// a clone of the same Arc so every caller sees the same in-flight
/// model load + download state.
pub fn shared() -> Arc<Mutex<LlmManager>> {
    SHARED
        .get_or_init(|| Arc::new(Mutex::new(LlmManager::new())))
        .clone()
}

/// Manages a local LLM for workspace orchestration.
pub struct LlmManager {
    backend: Option<LlamaBackend>,
    model: Option<LlamaModel>,
    model_path: Option<String>,
    pub downloading: AtomicBool,
}

// LlamaModel is Send but not marked as such in the crate.
// The llama.cpp library is thread-safe for inference after model load.
unsafe impl Send for LlmManager {}
unsafe impl Sync for LlmManager {}

impl LlmManager {
    /// Creates a new unloaded LLM manager.
    pub fn new() -> Self {
        Self {
            backend: None,
            model: None,
            model_path: None,
            downloading: AtomicBool::new(false),
        }
    }

    /// Loads a GGUF model from the given path.
    pub fn load_model(&mut self, path: &str) -> Result<(), String> {
        // Initialize backend if not already done
        if self.backend.is_none() {
            let backend = LlamaBackend::init()
                .map_err(|e| format!("Failed to initialize LLM backend: {e}"))?;
            self.backend = Some(backend);
        }

        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| "Backend not initialized".to_string())?;

        // Configure model params — use all GPU layers for Metal acceleration
        let model_params = LlamaModelParams::default().with_n_gpu_layers(1000);

        let model = LlamaModel::load_from_file(backend, path, &model_params)
            .map_err(|e| format!("Failed to load model from '{path}': {e}"))?;

        self.model = Some(model);
        self.model_path = Some(path.to_string());

        Ok(())
    }

    /// Load a model with CPU-only inference (no Metal GPU).
    /// Used by the worker subprocess to avoid Metal contention with the main app.
    pub fn load_model_cpu(&mut self, path: &str) -> Result<(), String> {
        if self.backend.is_none() {
            let backend = LlamaBackend::init()
                .map_err(|e| format!("Failed to initialize LLM backend: {e}"))?;
            self.backend = Some(backend);
        }

        let backend = self.backend.as_ref()
            .ok_or_else(|| "Backend not initialized".to_string())?;

        // Zero GPU layers = CPU only
        let model_params = LlamaModelParams::default().with_n_gpu_layers(0);

        let model = LlamaModel::load_from_file(backend, path, &model_params)
            .map_err(|e| format!("Failed to load model from '{path}': {e}"))?;

        self.model = Some(model);
        self.model_path = Some(path.to_string());
        Ok(())
    }

    /// Returns whether a model is currently loaded.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    /// Returns the configured model path, if any.
    pub fn get_model_path(&self) -> Option<String> {
        self.model_path.clone()
    }

    /// Explicitly unloads the model and backend to release Metal/GPU resources.
    /// Must be called before process exit to avoid SIGABRT from ggml_metal
    /// static destructors racing against Metal device teardown.
    pub fn unload(&mut self) {
        self.model = None;
        self.model_path = None;
        self.backend = None;
    }

    /// Returns whether a download is in progress.
    pub fn is_downloading(&self) -> bool {
        self.downloading.load(Ordering::Relaxed)
    }

    /// Runs inference with a system prompt and user message.
    /// Uses low temperature (0.1) for reliable tool-calling output.
    pub fn generate(&self, system_prompt: &str, user_message: &str) -> Result<String, String> {
        let model = self
            .model
            .as_ref()
            .ok_or_else(|| "No model loaded".to_string())?;
        let backend = self
            .backend
            .as_ref()
            .ok_or_else(|| "Backend not initialized".to_string())?;

        // Build chat messages
        let messages = vec![
            LlamaChatMessage::new("system".to_string(), system_prompt.to_string())
                .map_err(|e| format!("Failed to create system message: {e}"))?,
            LlamaChatMessage::new("user".to_string(), user_message.to_string())
                .map_err(|e| format!("Failed to create user message: {e}"))?,
        ];

        // Apply the model's chat template to format the prompt
        let chat_template = model
            .chat_template(None)
            .map_err(|e| format!("Failed to get chat template: {e}"))?;
        let prompt = model
            .apply_chat_template(&chat_template, &messages, true)
            .map_err(|e| format!("Failed to apply chat template: {e}"))?;

        // Create context — 6144 tokens to fit tools + settings schema + git tools
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(6144))
            .with_n_threads(4)
            .with_n_threads_batch(4);

        let mut ctx = model
            .new_context(backend, ctx_params)
            .map_err(|e| format!("Failed to create context: {e}"))?;

        // Tokenize the formatted prompt
        let tokens = model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| format!("Failed to tokenize prompt: {e}"))?;

        log_debug!("[llm] Prompt: {} tokens (max 3500)", tokens.len());

        if tokens.len() > 5500 {
            return Err(format!("Prompt too long: {} tokens (max 5500)", tokens.len()));
        }

        // Feed prompt tokens into the context via batch
        let mut batch = LlamaBatch::new(4096, 1);
        let last_idx = tokens.len() - 1;
        for (i, &token) in tokens.iter().enumerate() {
            batch
                .add(token, i as i32, &[0], i == last_idx)
                .map_err(|e| format!("Failed to add token to batch: {e}"))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| format!("Failed to decode prompt: {e}"))?;

        // Set up sampler: temperature 0.1 for deterministic tool-calling,
        // then top-p and dist for minimal randomness
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::temp(0.1),
            LlamaSampler::top_p(0.9, 1),
            LlamaSampler::dist(42),
        ]);

        // Generate tokens with a 30-second timeout to prevent hangs
        let mut output = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut n_cur = tokens.len() as i32;
        let max_tokens = 512; // Workspace commands are short
        let mut n_generated = 0;
        let generation_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

        while n_generated < max_tokens {
            if std::time::Instant::now() > generation_deadline {
                log_debug!("[llm] Generation timed out after 30s ({n_generated} tokens generated)");
                if output.is_empty() {
                    return Err("LLM generation timed out".to_string());
                }
                // Return what we have so far — partial output is better than nothing
                break;
            }
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);

            if model.is_eog_token(token) {
                break;
            }

            let piece = model
                .token_to_piece(token, &mut decoder, true, None)
                .map_err(|e| format!("Failed to convert token to text: {e}"))?;
            output.push_str(&piece);

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| format!("Failed to add generated token: {e}"))?;
            n_cur += 1;

            ctx.decode(&mut batch)
                .map_err(|e| format!("Failed to decode generated token: {e}"))?;

            n_generated += 1;
        }

        Ok(output.trim().to_string())
    }
}
