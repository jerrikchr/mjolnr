//! Read-only Jules connectivity commands.

use std::sync::Arc;

use clap::Subcommand;

use crate::core::secrets::SecretStore;

#[derive(Debug, Subcommand)]
pub enum JulesCommand {
    /// Verify the credential and list repositories connected to Jules.
    Sources,
}

pub fn run(command: JulesCommand, secrets: &Arc<dyn SecretStore>) -> i32 {
    match command {
        JulesCommand::Sources => {
            let client = match crate::integrations::jules::JulesClient::from_secret_store(
                secrets.as_ref(),
            ) {
                Ok(client) => client,
                Err(error) => {
                    eprintln!("mjolnr jules: {error}");
                    return 1;
                }
            };
            let result = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime.block_on(client.list_sources()),
                Err(error) => {
                    eprintln!("mjolnr jules: could not start network runtime: {error}");
                    return 1;
                }
            };
            match result {
                Ok(sources) => {
                    for source in sources {
                        println!("{}", source.name);
                    }
                    0
                }
                Err(error) => {
                    eprintln!("mjolnr jules: {error}");
                    1
                }
            }
        }
    }
}
