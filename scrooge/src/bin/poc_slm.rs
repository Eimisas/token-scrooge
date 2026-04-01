use anyhow::Result;
use scrooge::models::slm::Slm;

fn main() -> Result<()> {
    let home = dirs::home_dir().expect("no home dir");
    let cache_dir = home.join(".scrooge").join("models");
    
    println!("Loading Qwen2.5-0.5B-Instruct...");
    let mut slm = Slm::load("Qwen/Qwen2.5-0.5B-Instruct", cache_dir)?;
    
    let prompt = "Reconcile these conflicting facts into one truth: 
1. [2024-01-01] The project uses React 17.
2. [2024-06-01] We decided to migrate to React 18.
3. [2024-03-01] React 17 is the standard for now.";
    
    println!("\nPrompt: {}", prompt);
    println!("\nGenerating response...");
    let response = slm.generate(prompt, 100)?;
    
    println!("\nResponse:\n{}", response);
    
    Ok(())
}
