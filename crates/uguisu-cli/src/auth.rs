//! `uguisu auth`: the password, and the tokens that stand in for it.
//!
//! A password is read from the terminal without echoing it, or from standard
//! input when there is no terminal, and never from an argument or an
//! environment variable: an argument is in every process listing and a shell
//! history, and `UGUISU_TOKEN` exists for automation that already holds a
//! token rather than for handing over the password itself.

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uguisu_core::auth::Scope;
use uguisu_core::ids::ApiTokenId;
use uguisu_core::secret::Secret;

use crate::library::{client_exit, engine_exit, remote};
use crate::output::{self, Exit};
use crate::{Cli, open_engine};

/// Reads a password without echoing it.
///
/// Falls back to a plain line read when standard input is not a terminal, so a
/// script can pipe one in — which is how `scripts/e2e.py` sets one — while an
/// operator at a keyboard never sees it on screen.
fn read_password(prompt: &str) -> Result<Secret<String>, String> {
    let value = if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        rpassword::prompt_password(prompt).map_err(|e| e.to_string())?
    } else {
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line)
            .map_err(|e| e.to_string())?;
        line.trim_end_matches(['\n', '\r']).to_owned()
    };
    if value.is_empty() {
        return Err("no password was given".to_owned());
    }
    Ok(Secret::new(value))
}

/// `auth set-password`.
///
/// Embedded, the current password is not asked for. Anyone who can run this
/// already has the database and every archived file on disk, so requiring it
/// would protect nothing and would remove the only way back in after a
/// forgotten password. Over `--server` it is required, because there the
/// network is the boundary.
pub async fn run_set_password(cli: &Cli, username: Option<&str>) -> Exit {
    let command = "auth set-password";
    let remote_client = match remote(cli, command) {
        Ok(c) => c,
        Err(exit) => return exit,
    };
    if let Some(client) = remote_client {
        let current = match read_password("Current password: ") {
            Ok(p) => p,
            Err(e) => {
                output::error(cli.json, command, &e);
                return Exit::Usage;
            }
        };
        let new = match read_password("New password: ") {
            Ok(p) => p,
            Err(e) => {
                output::error(cli.json, command, &e);
                return Exit::Usage;
            }
        };
        let mut body = serde_json::json!({ "new_password": new.expose(), "current_password": current.expose() });
        if let Some(name) = username {
            body["username"] = serde_json::Value::String(name.to_owned());
        }
        return match client.auth_post("api/v1/auth/password", Some(body)).await {
            Ok(()) => {
                output::plain(cli.json, command, "Password set.");
                Exit::Ok
            }
            Err(e) => client_exit(cli, command, &e),
        };
    }

    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let existing = match engine.credential_username().await {
        Ok(u) => u,
        Err(e) => {
            let exit = engine_exit(cli, command, &e);
            engine.close().await;
            return exit;
        }
    };
    let new = match read_password("New password: ") {
        Ok(p) => p,
        Err(e) => {
            output::error(cli.json, command, &e);
            engine.close().await;
            return Exit::Usage;
        }
    };
    let name = username
        .map(str::to_owned)
        .or(existing)
        .unwrap_or_else(|| "uguisu".to_owned());
    let result = engine.set_password(&name, &new, None).await;
    engine.close().await;
    match result {
        Ok(revoked) => {
            output::plain(
                cli.json,
                command,
                &if revoked > 0 {
                    format!("Password set for {name}; {revoked} open sessions closed.")
                } else {
                    format!("Password set for {name}.")
                },
            );
            Exit::Ok
        }
        Err(e) => engine_exit(cli, command, &e),
    }
}

/// `auth token create <name>`.
pub async fn run_token_create(
    cli: &Cli,
    name: &str,
    scope: &str,
    expires_at: Option<&str>,
) -> Exit {
    let command = "auth token create";
    let Some(scope) = Scope::parse(scope) else {
        output::error(
            cli.json,
            command,
            &format!("`{scope}` is not a scope (read, write)"),
        );
        return Exit::Usage;
    };
    let mut deadline = None;
    if let Some(raw) = expires_at {
        let Ok(at) = OffsetDateTime::parse(raw, &Rfc3339) else {
            output::error(
                cli.json,
                command,
                &format!("`{raw}` is not an RFC 3339 timestamp"),
            );
            return Exit::Usage;
        };
        deadline = Some(at);
    }
    let expires_at = deadline;

    let remote_client = match remote(cli, command) {
        Ok(c) => c,
        Err(exit) => return exit,
    };
    if let Some(client) = remote_client {
        let mut body = serde_json::json!({ "name": name, "scope": scope.as_str() });
        if let Some(text) = expires_at.and_then(|at| at.format(&Rfc3339).ok()) {
            body["expires_at"] = serde_json::Value::String(text);
        }
        return match client
            .auth_post_json("api/v1/auth/tokens", Some(body))
            .await
        {
            Ok(value) => {
                output::token_created(
                    cli.json,
                    &value["token"],
                    value["secret"].as_str().unwrap_or_default(),
                );
                Exit::Ok
            }
            Err(e) => client_exit(cli, command, &e),
        };
    }

    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let result = engine.issue_token(name, scope, expires_at).await;
    engine.close().await;
    match result {
        Ok(issued) => {
            let token = serde_json::to_value(&issued.token).unwrap_or(serde_json::Value::Null);
            output::token_created(cli.json, &token, issued.secret.expose());
            Exit::Ok
        }
        Err(e) => engine_exit(cli, command, &e),
    }
}

/// `auth token list`.
pub async fn run_token_list(cli: &Cli) -> Exit {
    let command = "auth token list";
    let remote_client = match remote(cli, command) {
        Ok(c) => c,
        Err(exit) => return exit,
    };
    if let Some(client) = remote_client {
        return match client.auth_get_json("api/v1/auth/tokens").await {
            Ok(value) => {
                output::tokens(cli.json, &value["tokens"]);
                Exit::Ok
            }
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let result = engine.list_tokens().await;
    engine.close().await;
    match result {
        Ok(tokens) => {
            let value = serde_json::to_value(&tokens).unwrap_or(serde_json::Value::Null);
            output::tokens(cli.json, &value);
            Exit::Ok
        }
        Err(e) => engine_exit(cli, command, &e),
    }
}

/// `auth token revoke <id>`.
pub async fn run_token_revoke(cli: &Cli, raw: &str) -> Exit {
    let command = "auth token revoke";
    let Ok(id) = raw.trim().parse::<ApiTokenId>() else {
        output::error(
            cli.json,
            command,
            &format!("`{raw}` is not a token id (see `uguisu auth token list`)"),
        );
        return Exit::Usage;
    };
    let remote_client = match remote(cli, command) {
        Ok(c) => c,
        Err(exit) => return exit,
    };
    if let Some(client) = remote_client {
        return match client.auth_revoke_token(&id.to_string()).await {
            Ok(()) => {
                output::plain(cli.json, command, &format!("Revoked token {id}."));
                Exit::Ok
            }
            Err(e) => client_exit(cli, command, &e),
        };
    }
    let engine = match open_engine(cli, command).await {
        Ok(e) => e,
        Err(exit) => return exit,
    };
    let result = engine.revoke_token(id).await;
    engine.close().await;
    match result {
        Ok(true) => {
            output::plain(cli.json, command, &format!("Revoked token {id}."));
            Exit::Ok
        }
        Ok(false) => {
            output::error(cli.json, command, &format!("no open token {id}"));
            Exit::Usage
        }
        Err(e) => engine_exit(cli, command, &e),
    }
}
