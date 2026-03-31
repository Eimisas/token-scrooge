mod analytics;
mod cli;
mod config;
mod db;
mod embeddings;
mod error;
mod extract;
mod format;
mod hooks;
mod inject;
mod scoring;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.savings {
        return cli::cmd_gain();
    }

    match cli.command {
        Some(Commands::Claude { args })          => cli::cmd_claude(args),
        Some(Commands::Hook   { event })         => cli::cmd_hook(event),
        Some(Commands::Remember { text, tag })                          => cli::cmd_remember(text, tag),
        Some(Commands::Forget  { id })                                  => cli::cmd_forget(id),
        Some(Commands::Recall  { query, limit, include_archived })      => cli::cmd_recall(query, limit, include_archived),
        Some(Commands::Expire  { days, dry_run })                       => cli::cmd_expire(days, dry_run),
        Some(Commands::Gain)                                            => cli::cmd_gain(),
        Some(Commands::Setup)                                           => cli::cmd_setup(),
        Some(Commands::Init)                                            => cli::cmd_init(),
        Some(Commands::Uninstall { global })                            => cli::cmd_uninstall(global),
        Some(Commands::Config { action })                               => cli::cmd_config(action),
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            Ok(())
        }
    }
}
