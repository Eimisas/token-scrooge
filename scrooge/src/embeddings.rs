use anyhow::Result;
use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use hf_hub::{api::sync::ApiBuilder, Repo, RepoType};
use tokenizers::Tokenizer;

pub struct EmbeddingModel {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl EmbeddingModel {
    pub fn load() -> Result<Self> {
        let device = Device::Cpu;
        let repo = Repo::new("sentence-transformers/all-MiniLM-L6-v2".to_string(), RepoType::Model);
        
        let cache_dir = crate::config::global_scrooge_dir()?.join("models");
        let api = ApiBuilder::new()
            .with_endpoint("https://huggingface.co".to_string())
            .with_cache_dir(cache_dir)
            .build()?
            .repo(repo);
        
        let config_filename = api.get("config.json")?;
        let tokenizer_filename = api.get("tokenizer.json")?;
        let weights_filename = api.get("model.safetensors")?;

        let config = std::fs::read_to_string(config_filename)?;
        let config: Config = serde_json::from_str(&config)?;
        let tokenizer = Tokenizer::from_file(tokenizer_filename)
            .map_err(|e| anyhow::anyhow!("Tokenizer error: {}", e))?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_filename], candle_core::DType::F32, &device)?
        };
        let model = BertModel::load(vb, &config)?;

        Ok(Self { model, tokenizer, device })
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let tokens = self.tokenizer.encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenizer error: {}", e))?;
        let token_ids = tokens.get_ids();
        let token_ids = Tensor::new(token_ids, &self.device)?.unsqueeze(0)?;
        let token_type_ids = token_ids.zeros_like()?;

        let embeddings = self.model.forward(&token_ids, &token_type_ids, None)?;

        // Mean pooling
        let (_n_batch, n_tokens, _hidden_size) = embeddings.dims3()?;
        let embeddings = (embeddings.sum(1)? / (n_tokens as f64))?;
        let embeddings = embeddings.get(0)?;

        // Normalization (L2)
        let norm = embeddings.sqr()?.sum_all()?.sqrt()?;
        let embeddings = (embeddings / norm)?;

        Ok(embeddings.to_vec1::<f32>()?)
    }
}

pub fn cosine_similarity(v1: &[f32], v2: &[f32]) -> f32 {
    let mut dot = 0.0;
    for i in 0..v1.len() {
        dot += v1[i] * v2[i];
    }
    dot
}
