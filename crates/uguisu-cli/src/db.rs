//! `db migrate|backup|check|vacuum` (ADR 0056): embedded on the data
//! directory, or with `--server` against the running server, whose backups
//! go to its own `backups/` directory.

use std::path::Path;

use crate::library::{client_exit, engine_exit, remote};
use crate::output::{self, Exit};
use crate::{Cli, DbCommand, open_engine};

/// Runs one `db` command.
pub async fn run(cli: &Cli, action: &DbCommand) -> Exit {
    match action {
        DbCommand::Migrate => migrate(cli).await,
        DbCommand::Backup { path } => backup(cli, path.as_deref()).await,
        DbCommand::Check => check(cli).await,
        DbCommand::Vacuum => vacuum(cli).await,
    }
}

#[allow(clippy::print_stdout)]
async fn migrate(cli: &Cli) -> Exit {
    const CMD: &str = "db migrate";
    if cli.server.is_some() {
        output::error(
            cli.json,
            CMD,
            "a server applies its migrations when it starts; `db migrate` runs on a data directory",
        );
        return Exit::Usage;
    }
    let engine = match open_engine(cli, CMD).await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let schema = engine.storage().schema().clone();
    engine.close().await;
    if cli.json {
        output::json_with_schema(&schema);
    } else if schema.applied.is_empty() {
        println!("Schema {}; nothing to apply", schema.now);
    } else {
        let applied: Vec<String> = schema.applied.iter().map(ToString::to_string).collect();
        println!(
            "Schema {} -> {} (applied {})",
            schema
                .found
                .map_or_else(|| "none".to_owned(), |v| v.to_string()),
            schema.now,
            applied.join(", ")
        );
    }
    Exit::Ok
}

#[allow(clippy::print_stdout)]
async fn backup(cli: &Cli, path: Option<&Path>) -> Exit {
    const CMD: &str = "db backup";
    let backup = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => {
            if path.is_some() {
                output::error(
                    cli.json,
                    CMD,
                    "with --server the backup goes to the server's backups/ directory; leave out the path",
                );
                return Exit::Usage;
            }
            match client.db_backup().await {
                Ok(b) => b,
                Err(e) => return client_exit(cli, CMD, &e),
            }
        }
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.backup_database(path).await;
            engine.close().await;
            match result {
                Ok(b) => b,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&backup);
    } else {
        println!(
            "Backed up the database to {} ({} bytes)",
            backup.path, backup.bytes
        );
    }
    Exit::Ok
}

#[allow(clippy::print_stdout)]
async fn check(cli: &Cli) -> Exit {
    const CMD: &str = "db check";
    let check = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.db_check().await {
            Ok(c) => c,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.check_database().await;
            engine.close().await;
            match result {
                Ok(c) => c,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&check);
    } else if check.ok {
        println!("The database is sound");
    } else {
        for line in &check.integrity {
            println!("integrity: {line}");
        }
        for line in &check.foreign_keys {
            println!("foreign key: {line}");
        }
    }
    if check.ok { Exit::Ok } else { Exit::Error }
}

#[allow(clippy::print_stdout)]
async fn vacuum(cli: &Cli) -> Exit {
    const CMD: &str = "db vacuum";
    let vacuum = match remote(cli, CMD) {
        Err(exit) => return exit,
        Ok(Some(client)) => match client.db_vacuum().await {
            Ok(v) => v,
            Err(e) => return client_exit(cli, CMD, &e),
        },
        Ok(None) => {
            let engine = match open_engine(cli, CMD).await {
                Ok(e) => e,
                Err(exit) => return exit,
            };
            let result = engine.vacuum_database().await;
            engine.close().await;
            match result {
                Ok(v) => v,
                Err(e) => return engine_exit(cli, CMD, &e),
            }
        }
    };
    if cli.json {
        output::json_with_schema(&vacuum);
    } else {
        println!(
            "Vacuumed the database: {} -> {} bytes",
            vacuum.bytes_before, vacuum.bytes_after
        );
    }
    Exit::Ok
}
