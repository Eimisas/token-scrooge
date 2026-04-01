use anyhow::{Result, Error};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::qwen2::{Config, Model};
use candle_transformers::generation::LogitsProcessor;
use hf_hub::{api::sync::ApiBuilder, Repo, RepoType};
use tokenizers::Tokenizer;
use std::path::PathBuf;

pub struct Slm {
    model: Model,
    tokenizer: Tokenizer,
    device: Device,
    logits_processor: LogitsProcessor,
}

impl Slm {
    pub fn load(model_id: &str, cache_dir: PathBuf) -> Result<Self> {
        let device = Device::Cpu; // Default to CPU for PoC
        
        let api = ApiBuilder::new()
            .with_endpoint("https://huggingface.co".to_string())
            .with_cache_dir(cache_dir)
            .build()?
            .repo(Repo::new(model_id.to_string(), RepoType::Model));
            
        let config_filename = api.get("config.json")?;
        let tokenizer_filename = api.get("tokenizer.json")?;
        
        // Qwen2.5 models are usually split or single safetensors
        // For 0.5B, it is usually a single one.
        let weights_filename = api.get("model.safetensors")?;

        let config: Config = serde_json::from_str(&std::fs::read_to_string(config_filename)?)?;
        let tokenizer = Tokenizer::from_file(tokenizer_filename).map_err(Error::msg)?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_filename], DType::F32, &device)?
        };
        let model = Model::new(&config, vb)?;

        Ok(Self {
            model,
            tokenizer,
            device,
            logits_processor: LogitsProcessor::new(1337, Some(0.1), None),
        })
    }

    pub fn generate(&mut self, prompt: &str, max_tokens: usize) -> Result<String> {
        // Qwen2.5-Instruct chat template
        let formatted_prompt = format!(
            "<|im_start|>system\nYou are a precise technical assistant. Follow instructions exactly. Output only what is requested — no explanations, no prose, no markdown fences.<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
            prompt
        );

        let tokens = self.tokenizer.encode(formatted_prompt, true).map_err(Error::msg)?;
        let mut tokens = tokens.get_ids().to_vec();
        let mut generated_text = String::new();
        let mut index_pos = 0;

        for _ in 0..max_tokens {
            let input = Tensor::new(&tokens[index_pos..], &self.device)?.unsqueeze(0)?;
            let logits = self.model.forward(&input, index_pos, None)?;
            let logits = logits.squeeze(0)?;
            let logits = logits.get(logits.dim(0)? - 1)?;
            let next_token = self.logits_processor.sample(&logits)?;

            index_pos = tokens.len();
            tokens.push(next_token);

            let decoded = self.tokenizer.decode(&[next_token], true).map_err(Error::msg)?;
            if decoded.contains("<|im_end|>") || decoded.contains("<|endoftext|>") {
                break;
            }
            generated_text.push_str(&decoded);
        }

        Ok(generated_text)
    }
}
