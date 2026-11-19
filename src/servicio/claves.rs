//! Claves de API para integradores. Cualquier cliente puede crear las suyas desde
//! la app («Desarrolladores»): lo que cobra con ellas entra a su cuenta.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::vistas::{ClaveApi, ClaveCreada};
use super::Banco;
use crate::error::ErrorBanco;
use crate::infra::seguridad;

pub const CLAVES_ACTIVAS_MAXIMAS: i64 = 10;
const PREFIJO_CLAVE: &str = "vm_test_";
const PREFIJO_SECRETO: &str = "whsec_";

/// Quién llama a la API de integración (sale de `X-Api-Key`).
#[derive(Debug, Clone, Copy)]
pub struct Integrador {
    pub clave_id: Uuid,
    pub cliente_id: Uuid,
    pub cuenta_id: Uuid,
}

type FilaClave = (
    Uuid,
    String,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

const COLUMNAS_CLAVE: &str =
    "id, name, prefix, market_account_id, webhook_url, created_at, revoked_at";

fn a_vista(
    (id, name, prefix, account_id, webhook_url, created_at, revoked_at): FilaClave,
) -> ClaveApi {
    ClaveApi {
        id,
        name,
        prefix,
        account_id,
        webhook_url,
        created_at,
        revoked_at,
    }
}

/// Solo http(s); el resto de la URL lo valida quien la usa.
pub fn validar_webhook(url: Option<String>) -> Result<Option<String>, ErrorBanco> {
    let Some(url) = url.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    let esquema_ok = url.starts_with("http://") || url.starts_with("https://");
    if !esquema_ok || url.len() > 2048 || url.chars().any(char::is_whitespace) {
        return Err(ErrorBanco::validacion(
            "webhookUrl",
            "URL http(s) de hasta 2048 caracteres",
        ));
    }
    Ok(Some(url))
}

fn validar_nombre(nombre: &str) -> Result<String, ErrorBanco> {
    let nombre = nombre.trim().to_owned();
    if !(1..=60).contains(&nombre.chars().count()) {
        return Err(ErrorBanco::validacion("name", "entre 1 y 60 caracteres"));
    }
    Ok(nombre)
}

impl Banco {
    pub async fn crear_clave(
        &self,
        cliente_id: Uuid,
        nombre: &str,
        webhook_url: Option<String>,
    ) -> Result<ClaveCreada, ErrorBanco> {
        let nombre = validar_nombre(nombre)?;
        let webhook_url = validar_webhook(webhook_url)?;
        let activas: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM api_keys WHERE customer_id = $1 AND revoked_at IS NULL",
        )
        .bind(cliente_id)
        .fetch_one(self.pool())
        .await?;
        if activas >= CLAVES_ACTIVAS_MAXIMAS {
            return Err(ErrorBanco::LimiteDeClaves(CLAVES_ACTIVAS_MAXIMAS));
        }
        let api_key = seguridad::token(PREFIJO_CLAVE);
        let secreto = seguridad::token(PREFIJO_SECRETO);
        let clave = self
            .guardar_clave(
                cliente_id,
                &nombre,
                &api_key,
                &seguridad::identificador_market(),
                webhook_url,
                &secreto,
            )
            .await?;
        Ok(ClaveCreada {
            clave,
            authorization_id: api_key.clone(),
            api_key,
            webhook_secret: secreto,
        })
    }

    /// Inserta o, si la clave ya existía (semilla), actualiza nombre y webhook.
    /// Las credenciales de la API Market de una clave existente no se tocan.
    pub(crate) async fn guardar_clave(
        &self,
        cliente_id: Uuid,
        nombre: &str,
        api_key: &str,
        account_id: &str,
        webhook_url: Option<String>,
        secreto: &str,
    ) -> Result<ClaveApi, ErrorBanco> {
        let prefijo: String = api_key.chars().take(PREFIJO_CLAVE.len() + 6).collect();
        let fila: FilaClave = sqlx::query_as(&format!(
            "INSERT INTO api_keys (id, customer_id, name, prefix, key_hash, webhook_url,
                                   webhook_secret, market_account_id, market_secret_hash)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $5)
             ON CONFLICT (key_hash) DO UPDATE
                 SET name = EXCLUDED.name, webhook_url = EXCLUDED.webhook_url,
                     webhook_secret = EXCLUDED.webhook_secret, revoked_at = NULL
             RETURNING {COLUMNAS_CLAVE}"
        ))
        .bind(Uuid::new_v4())
        .bind(cliente_id)
        .bind(nombre)
        .bind(prefijo)
        .bind(seguridad::sha256(api_key))
        .bind(webhook_url)
        .bind(secreto)
        .bind(account_id)
        .fetch_one(self.pool())
        .await?;
        Ok(a_vista(fila))
    }

    pub async fn listar_claves(&self, cliente_id: Uuid) -> Result<Vec<ClaveApi>, ErrorBanco> {
        let filas: Vec<FilaClave> = sqlx::query_as(&format!(
            "SELECT {COLUMNAS_CLAVE}
             FROM api_keys WHERE customer_id = $1
             ORDER BY revoked_at IS NOT NULL, created_at DESC"
        ))
        .bind(cliente_id)
        .fetch_all(self.pool())
        .await?;
        Ok(filas.into_iter().map(a_vista).collect())
    }

    pub async fn cambiar_webhook(
        &self,
        cliente_id: Uuid,
        clave_id: Uuid,
        webhook_url: Option<String>,
    ) -> Result<ClaveApi, ErrorBanco> {
        let webhook_url = validar_webhook(webhook_url)?;
        let fila: Option<FilaClave> = sqlx::query_as(&format!(
            "UPDATE api_keys SET webhook_url = $3
             WHERE id = $1 AND customer_id = $2 AND revoked_at IS NULL
             RETURNING {COLUMNAS_CLAVE}"
        ))
        .bind(clave_id)
        .bind(cliente_id)
        .bind(webhook_url)
        .fetch_optional(self.pool())
        .await?;
        fila.map(a_vista).ok_or(ErrorBanco::ClaveNoEncontrada)
    }

    pub async fn revocar_clave(&self, cliente_id: Uuid, clave_id: Uuid) -> Result<(), ErrorBanco> {
        let afectadas = sqlx::query(
            "UPDATE api_keys SET revoked_at = coalesce(revoked_at, now())
             WHERE id = $1 AND customer_id = $2",
        )
        .bind(clave_id)
        .bind(cliente_id)
        .execute(self.pool())
        .await?
        .rows_affected();
        if afectadas == 0 {
            return Err(ErrorBanco::ClaveNoEncontrada);
        }
        Ok(())
    }

    pub async fn autenticar_clave(&self, api_key: &str) -> Result<Option<Integrador>, ErrorBanco> {
        let fila: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "SELECT k.id, k.customer_id, a.id
             FROM api_keys k JOIN accounts a ON a.customer_id = k.customer_id
             WHERE k.key_hash = $1 AND k.revoked_at IS NULL",
        )
        .bind(seguridad::sha256(api_key))
        .fetch_optional(self.pool())
        .await?;
        Ok(fila.map(|(clave_id, cliente_id, cuenta_id)| Integrador {
            clave_id,
            cliente_id,
            cuenta_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_acepta_webhooks_http() {
        assert_eq!(validar_webhook(None).unwrap(), None);
        assert_eq!(validar_webhook(Some("  ".into())).unwrap(), None);
        assert!(validar_webhook(Some("http://sales:8080/x".into())).is_ok());
        assert!(validar_webhook(Some("ftp://x".into())).is_err());
        assert!(validar_webhook(Some("http://a b".into())).is_err());
    }
}
