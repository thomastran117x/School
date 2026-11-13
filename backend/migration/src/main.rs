//! Command-line entry point for applying and inspecting SeaORM migrations.
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

use std::{env, path::PathBuf};

use sea_orm_migration::sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DbErr, sqlx::mysql::MySqlSslMode,
};

#[tokio::main]
/// Runs the SeaORM migration CLI with the application's MySQL TLS policy.
async fn main() {
    sea_orm_migration::cli::run_cli_with_connection(migration::Migrator, connect_with_tls).await;
}

async fn connect_with_tls(mut options: ConnectOptions) -> Result<DatabaseConnection, DbErr> {
    if database_url_has_tls_options(options.get_url()) {
        return Err(DbErr::Custom(
            "DATABASE_URL must not contain TLS options; use the dedicated TLS variables".to_owned(),
        ));
    }
    let tls_mode = match env::var("DATABASE_TLS_MODE")
        .unwrap_or_else(|_| "verify_identity".to_owned())
        .to_ascii_lowercase()
        .as_str()
    {
        "disabled" => MySqlSslMode::Disabled,
        "preferred" => MySqlSslMode::Preferred,
        "required" => MySqlSslMode::Required,
        "verify_ca" => MySqlSslMode::VerifyCa,
        "verify_identity" => MySqlSslMode::VerifyIdentity,
        _ => {
            return Err(DbErr::Custom(
                "DATABASE_TLS_MODE has an invalid value".to_owned(),
            ));
        }
    };
    let tls_ca_path = env::var("DATABASE_TLS_CA_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    if tls_ca_path.is_some()
        && !matches!(
            tls_mode,
            MySqlSslMode::VerifyCa | MySqlSslMode::VerifyIdentity
        )
    {
        return Err(DbErr::Custom(
            "DATABASE_TLS_CA_PATH requires certificate verification".to_owned(),
        ));
    }
    if tls_ca_path
        .as_ref()
        .is_some_and(|path| !path.is_file() || std::fs::File::open(path).is_err())
    {
        return Err(DbErr::Custom(
            "DATABASE_TLS_CA_PATH does not identify a readable file".to_owned(),
        ));
    }

    options.map_sqlx_mysql_opts(move |options| {
        let options = options.ssl_mode(tls_mode);
        match tls_ca_path.as_ref() {
            Some(path) => options.ssl_ca(path),
            None => options,
        }
    });
    Database::connect(options).await
}

fn database_url_has_tls_options(url: &str) -> bool {
    url.split_once('?').is_some_and(|(_, query)| {
        query.split('&').any(|pair| {
            let key = pair.split_once('=').map_or(pair, |(key, _)| key);
            matches!(
                key.to_ascii_lowercase().as_str(),
                "ssl-mode" | "sslmode" | "ssl-ca" | "sslca"
            )
        })
    })
}
