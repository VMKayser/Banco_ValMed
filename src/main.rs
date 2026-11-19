//! Arranque: configuración, PostgreSQL, migraciones, datos semilla, trabajador de
//! avisos y servidor HTTP.
//!
//! Subcomandos:
//! - `banco-valmed healthcheck`: GET a `/health`, termina con 0 o 1 (la imagen no trae curl).
//! - `banco-valmed openapi`: imprime el contrato OpenAPI en YAML.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use banco_valmed::config::{self, Config};
use banco_valmed::infra::seguridad;
use banco_valmed::servicio::{avisos, semilla};
use banco_valmed::{api, Banco};

#[tokio::main]
async fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("healthcheck") => return healthcheck(),
        Some("openapi") => {
            print!("{}", api::contrato_yaml()?);
            return Ok(());
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_current_span(true)
        .with_span_list(false)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let config = Config::desde_entorno()?;
    LazyLock::force(&seguridad::HASH_FICTICIO);
    if !config.secreto_tokens.fijo {
        tracing::warn!(
            "BANK_TOKEN_SECRET vacío: los tokens de la API Market dejan de servir al reiniciar"
        );
    }

    let pool = conectar(&config.database_url).await?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("no se pudieron aplicar las migraciones")?;

    let puerto = config.puerto_http;
    let banco = Banco::new(pool, config)?;
    semilla::sembrar(&banco)
        .await
        .context("no se pudieron cargar los datos semilla")?;
    tokio::spawn(avisos::trabajador(banco.clone()));

    let oyente = TcpListener::bind(("0.0.0.0", puerto)).await?;
    tracing::info!(
        puerto,
        sandbox = banco.config().sandbox,
        url_publica = banco
            .config()
            .url_publica
            .as_deref()
            .unwrap_or("(desde cada petición)"),
        "Banco ValMed escuchando"
    );
    axum::serve(oyente, api::router(banco))
        .with_graceful_shutdown(senal_de_apagado())
        .await
        .context("servidor HTTP")?;
    tracing::info!("Banco ValMed detenido");
    Ok(())
}

/// Reintenta mientras PostgreSQL arranca (en Compose también hay `depends_on`).
async fn conectar(url: &str) -> Result<PgPool> {
    let opciones = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(3));
    let mut intento = 1;
    loop {
        match opciones.clone().connect(url).await {
            Ok(pool) => return Ok(pool),
            Err(e) if intento < 15 => {
                tracing::warn!(intento, error = %e, "PostgreSQL no responde; reintento en 2 s");
                tokio::time::sleep(Duration::from_secs(2)).await;
                intento += 1;
            }
            Err(e) => return Err(e).context("no se pudo conectar a PostgreSQL"),
        }
    }
}

/// Ctrl+C o SIGTERM (`docker stop`): deja de aceptar conexiones y termina las
/// peticiones en curso. Los streams SSE abiertos se cortan.
async fn senal_de_apagado() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminar = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminar = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminar => {},
    }
}

fn healthcheck() -> Result<()> {
    let puerto = config::puerto_desde_entorno()?;
    let mut conexion =
        TcpStream::connect_timeout(&([127, 0, 0, 1], puerto).into(), Duration::from_secs(2))?;
    conexion.set_read_timeout(Some(Duration::from_secs(3)))?;
    conexion.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut respuesta = String::new();
    conexion.read_to_string(&mut respuesta)?;
    if respuesta.starts_with("HTTP/1.1 200") || respuesta.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        anyhow::bail!(
            "health respondió: {}",
            respuesta.lines().next().unwrap_or("")
        )
    }
}
