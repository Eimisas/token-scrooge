use anyhow::Result;
use clap::Parser;
use scrooge::cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.savings {
        return scrooge::cli::cmd_gain();
    }

    match cli.command {
        Some(Commands::Claude { args })          => scrooge::cli::cmd_claude(args),
        Some(Commands::Hook   { event })         => scrooge::cli::cmd_hook(event),
        Some(Commands::Remember { text, tag })                          => scrooge::cli::cmd_remember(text, tag),
        Some(Commands::Forget  { id })                                  => scrooge::cli::cmd_forget(id),
        Some(Commands::Recall  { query, limit, include_archived })      => scrooge::cli::cmd_recall(query, limit, include_archived),
        Some(Commands::Expire  { days, dry_run })                       => scrooge::cli::cmd_expire(days, dry_run),
        Some(Commands::Gain)                                            => scrooge::cli::cmd_gain(),
        Some(Commands::Setup)                                           => scrooge::cli::cmd_setup(),
        Some(Commands::Init)                                            => scrooge::cli::cmd_init(),
        Some(Commands::Uninstall { global })                            => scrooge::cli::cmd_uninstall(global),
        Some(Commands::Config { action })                               => scrooge::cli::cmd_config(action),
        Some(Commands::Daemon { action })                               => scrooge::cli::cmd_daemon(action),
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            Ok(())
        }
    }
}
