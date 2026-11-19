//! Casos de uso del banco. Cada operación que mueve dinero es una sola
//! transacción de PostgreSQL con las cuentas bloqueadas en orden de id.

pub mod avisos;
pub mod claves;
pub mod clientes;
pub mod cobros;
pub mod market;
pub mod semilla;
pub mod vistas;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::config::Config;
use vistas::EstadoCobro;

/// Cambio de estado de un cobro QR; lo escuchan los streams SSE.
#[derive(Debug, Clone, Copy)]
pub struct EventoQr {
    pub id: Uuid,
    pub estado: EstadoCobro,
}

#[derive(Clone)]
pub struct Banco {
    interno: Arc<Interno>,
}

struct Interno {
    pool: PgPool,
    config: Config,
    eventos: broadcast::Sender<EventoQr>,
    http: reqwest::Client,
}

impl Banco {
    pub fn new(pool: PgPool, config: Config) -> Result<Self> {
        let (eventos, _) = broadcast::channel(1024);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .user_agent("BancoValMed-Webhooks/1.0")
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            interno: Arc::new(Interno {
                pool,
                config,
                eventos,
                http,
            }),
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.interno.pool
    }

    pub fn config(&self) -> &Config {
        &self.interno.config
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.interno.http
    }

    pub fn suscribir(&self) -> broadcast::Receiver<EventoQr> {
        self.interno.eventos.subscribe()
    }

    /// Sin suscriptores `send` falla; da igual, nadie estaba mirando.
    pub(crate) fn publicar(&self, id: Uuid, estado: EstadoCobro) {
        let _ = self.interno.eventos.send(EventoQr { id, estado });
    }
}

/// Bloquea las cuentas en orden de id para que dos operaciones cruzadas
/// (A paga a B mientras B paga a A) no se bloqueen entre sí.
pub(crate) async fn bloquear_cuentas(
    tx: &mut sqlx::PgConnection,
    cuentas: &[Uuid],
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT id FROM accounts WHERE id = ANY($1) ORDER BY id FOR UPDATE")
        .bind(cuentas)
        .fetch_all(tx)
        .await?;
    Ok(())
}

/// `1001-000042` → `****0042`.
pub fn enmascarar_cuenta(numero: &str) -> String {
    let digitos: String = numero.chars().filter(char::is_ascii_digit).collect();
    let cola = &digitos[digitos.len().saturating_sub(4)..];
    format!("****{cola}")
}
