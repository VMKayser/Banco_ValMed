//! API Market (Pago QR) del BNB sobre los mismos cobros del banco: credenciales
//! `accountId` + `authorizationId`, token Bearer (JWT) e id numérico del QR.

use std::collections::HashMap;

use chrono::{NaiveDate, Utc};
use uuid::Uuid;

use super::claves::Integrador;
use super::cobros::{FilaCobro, SELECT_COBRO};
use super::Banco;
use crate::error::ErrorBanco;
use crate::infra::jwt::{self, Reclamos};
use crate::infra::seguridad;

/// Vida del token: al vencer, responde 401 y hay que pedir otro.
pub const VIGENCIA_TOKEN_S: i64 = 3600;

/// Máximo de QR que devuelve `getQRbyGenerationDateAsync` para un día.
pub const QR_POR_DIA_MAXIMO: i64 = 1000;

/// Estado de la notificación de pago de un QR: (entregada, respuesta).
pub type EstadoNotificacion = (bool, Option<String>);

impl Banco {
    /// Token si `accountId` y `authorizationId` son de una clave activa.
    pub async fn token_market(
        &self,
        account_id: &str,
        authorization_id: &str,
    ) -> Result<Option<String>, ErrorBanco> {
        let fila: Option<(Uuid, String, String)> = sqlx::query_as(
            "SELECT k.id, c.full_name, a.number
             FROM api_keys k
             JOIN customers c ON c.id = k.customer_id
             JOIN accounts a ON a.customer_id = k.customer_id
             WHERE k.market_account_id = $1 AND k.market_secret_hash = $2
               AND k.revoked_at IS NULL",
        )
        .bind(account_id)
        .bind(seguridad::sha256(authorization_id))
        .fetch_optional(self.pool())
        .await?;
        Ok(fila.map(|(clave_id, nombre, cuenta)| {
            let reclamos = Reclamos::new(
                clave_id,
                &nombre,
                &cuenta,
                Utc::now().timestamp(),
                VIGENCIA_TOKEN_S,
            );
            jwt::emitir(self.config().secreto_tokens.bytes(), &reclamos)
        }))
    }

    /// Integrador dueño del token, si el token es válido y la clave sigue activa.
    pub async fn integrador_market(&self, token: &str) -> Result<Option<Integrador>, ErrorBanco> {
        let secreto = self.config().secreto_tokens.bytes();
        let Some(reclamos) = jwt::verificar(secreto, token, Utc::now().timestamp()) else {
            return Ok(None);
        };
        let fila: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "SELECT k.id, k.customer_id, a.id
             FROM api_keys k JOIN accounts a ON a.customer_id = k.customer_id
             WHERE k.id = $1 AND k.revoked_at IS NULL",
        )
        .bind(reclamos.sub)
        .fetch_optional(self.pool())
        .await?;
        Ok(fila.map(|(clave_id, cliente_id, cuenta_id)| Integrador {
            clave_id,
            cliente_id,
            cuenta_id,
        }))
    }

    /// Cambia el `authorizationId`. `false` si las credenciales actuales no son
    /// correctas. `X-Api-Key` sigue siendo la misma.
    pub async fn cambiar_autorizacion_market(
        &self,
        account_id: &str,
        actual: &str,
        nueva: &str,
    ) -> Result<bool, ErrorBanco> {
        let cambiadas = sqlx::query(
            "UPDATE api_keys SET market_secret_hash = $3
             WHERE market_account_id = $1 AND market_secret_hash = $2 AND revoked_at IS NULL",
        )
        .bind(account_id)
        .bind(seguridad::sha256(actual))
        .bind(seguridad::sha256(nueva))
        .execute(self.pool())
        .await?
        .rows_affected();
        Ok(cambiadas == 1)
    }

    /// Id interno de un QR por su id numérico (para el sandbox).
    pub async fn id_por_numero(&self, numero: i64) -> Result<Option<Uuid>, ErrorBanco> {
        Ok(
            sqlx::query_scalar("SELECT id FROM qr_charges WHERE number = $1")
                .bind(numero)
                .fetch_optional(self.pool())
                .await?,
        )
    }

    /// Un QR de esta cuenta por su id numérico.
    pub(crate) async fn cobro_por_numero(
        &self,
        cuenta_id: Uuid,
        numero: i64,
    ) -> Result<Option<FilaCobro>, ErrorBanco> {
        Ok(sqlx::query_as::<_, FilaCobro>(&format!(
            "{SELECT_COBRO} WHERE q.number = $1 AND q.account_id = $2"
        ))
        .bind(numero)
        .bind(cuenta_id)
        .fetch_optional(self.pool())
        .await?)
    }

    /// QR de esta cuenta creados ese día (hora de Bolivia), con el estado de su
    /// notificación de pago.
    pub(crate) async fn cobros_del_dia(
        &self,
        cuenta_id: Uuid,
        dia: NaiveDate,
    ) -> Result<Vec<(FilaCobro, Option<EstadoNotificacion>)>, ErrorBanco> {
        let filas = sqlx::query_as::<_, FilaCobro>(&format!(
            "{SELECT_COBRO}
             WHERE q.account_id = $1 AND (q.created_at AT TIME ZONE 'America/La_Paz')::date = $2
             ORDER BY q.number
             LIMIT $3"
        ))
        .bind(cuenta_id)
        .bind(dia)
        .bind(QR_POR_DIA_MAXIMO)
        .fetch_all(self.pool())
        .await?;

        let ids: Vec<Uuid> = filas.iter().map(|f| f.id).collect();
        let avisos: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
            "SELECT DISTINCT ON (qr_charge_id) qr_charge_id, status, last_response
             FROM webhook_deliveries
             WHERE qr_charge_id = ANY($1) AND event_type = 'qr.paid'
             ORDER BY qr_charge_id, created_at DESC",
        )
        .bind(&ids)
        .fetch_all(self.pool())
        .await?;
        let mut por_cobro: HashMap<Uuid, EstadoNotificacion> = avisos
            .into_iter()
            .map(|(id, estado, respuesta)| (id, (estado == "DELIVERED", respuesta)))
            .collect();

        Ok(filas
            .into_iter()
            .map(|f| {
                let aviso = por_cobro.remove(&f.id);
                (f, aviso)
            })
            .collect())
    }
}
