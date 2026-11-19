//! Envío de avisos firmados (webhooks) desde el outbox `webhook_deliveries`.
//!
//! Un trabajador toma lotes de avisos pendientes con `FOR UPDATE SKIP LOCKED` y
//! los «alquila» 30 s (si el proceso muere a mitad, otro los vuelve a tomar).
//! Cada intento usa los mismos bytes de cuerpo, con timestamp y firma nuevos.
//!
//! Formato `API_MARKET` (como el BNB): además del 2xx, el cuerpo de la respuesta
//! debe traer `"success": true`; si no, se reintenta.

use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use uuid::Uuid;

use super::vistas::Aviso;
use super::Banco;
use crate::dominio::firma;
use crate::error::ErrorBanco;

/// Espera después del intento n (1, 2, …). Tras el último, el aviso queda FAILED.
pub const ESPERAS_S: [i64; 6] = [5, 15, 60, 300, 900, 3600];
pub const INTENTOS_MAXIMOS: i32 = ESPERAS_S.len() as i32 + 1;

/// Qué hacer con la respuesta del integrador.
#[derive(Debug, PartialEq, Eq)]
pub enum Resultado {
    Entregado,
    Reintentar,
    Fallido,
}

/// 2xx entregado; 408, 429, 5xx o error de red se reintentan; otro 4xx falla
/// sin reintentar (el integrador tiene que corregir algo).
pub fn clasificar(estado: Option<u16>) -> Resultado {
    match estado {
        Some(200..=299) => Resultado::Entregado,
        None | Some(408) | Some(429) | Some(500..=599) => Resultado::Reintentar,
        Some(_) => Resultado::Fallido,
    }
}

/// Como el BNB: un 2xx solo cuenta si responde `{"success": true, ...}`.
pub fn clasificar_market(estado: Option<u16>, cuerpo: &str) -> Resultado {
    match clasificar(estado) {
        Resultado::Entregado if !respondio_exito(cuerpo) => Resultado::Reintentar,
        otro => otro,
    }
}

fn respondio_exito(cuerpo: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(cuerpo)
        .ok()
        .and_then(|v| {
            v.as_object()?
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("success"))
                .and_then(|(_, v)| v.as_bool())
        })
        .unwrap_or(false)
}

/// Hasta 500 caracteres de la respuesta, para mostrarla en «Avisos».
const LARGO_RESPUESTA: usize = 500;

pub async fn trabajador(banco: Banco) {
    tracing::info!("trabajador de avisos iniciado");
    loop {
        match procesar_lote(&banco).await {
            Ok(0) => tokio::time::sleep(Duration::from_secs(1)).await,
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "no se pudo procesar el lote de avisos");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn procesar_lote(banco: &Banco) -> Result<usize, sqlx::Error> {
    let lote: Vec<(Uuid, String, String, String, i32, String, String)> = sqlx::query_as(
        "UPDATE webhook_deliveries d SET next_attempt_at = now() + interval '30 seconds'
         FROM api_keys k
         WHERE d.id IN (SELECT id FROM webhook_deliveries
                        WHERE status = 'PENDING' AND next_attempt_at <= now()
                        ORDER BY next_attempt_at
                        LIMIT 10
                        FOR UPDATE SKIP LOCKED)
           AND k.id = d.api_key_id
         RETURNING d.id, d.event_type, d.url, d.body, d.attempts, k.webhook_secret, d.format",
    )
    .fetch_all(banco.pool())
    .await?;
    let cantidad = lote.len();
    join_all(lote.into_iter().map(
        |(id, tipo, url, cuerpo, intentos, secreto, formato)| async move {
            let aviso = Pendiente {
                id,
                tipo: &tipo,
                url: &url,
                cuerpo: &cuerpo,
                intentos_previos: intentos,
                secreto: &secreto,
                api_market: formato == "API_MARKET",
            };
            if let Err(e) = enviar(banco, aviso).await {
                tracing::warn!(aviso = %id, error = %e, "no se pudo guardar el resultado del aviso");
            }
        },
    ))
    .await;
    Ok(cantidad)
}

struct Pendiente<'a> {
    id: Uuid,
    tipo: &'a str,
    url: &'a str,
    cuerpo: &'a str,
    intentos_previos: i32,
    secreto: &'a str,
    api_market: bool,
}

/// Lee como mucho `LARGO_RESPUESTA` caracteres sin bajar todo el cuerpo.
async fn leer_inicio(mut respuesta: reqwest::Response) -> String {
    let mut bytes = Vec::new();
    while bytes.len() < LARGO_RESPUESTA * 4 {
        match respuesta.chunk().await {
            Ok(Some(trozo)) => bytes.extend_from_slice(&trozo),
            _ => break,
        }
    }
    String::from_utf8_lossy(&bytes)
        .chars()
        .take(LARGO_RESPUESTA)
        .collect()
}

async fn enviar(banco: &Banco, aviso: Pendiente<'_>) -> Result<(), sqlx::Error> {
    let Pendiente {
        id,
        tipo,
        url,
        cuerpo,
        intentos_previos,
        secreto,
        api_market,
    } = aviso;
    let timestamp = Utc::now().timestamp();
    let respuesta = banco
        .http()
        .post(url)
        .header("Content-Type", "application/json")
        .header("X-ValMed-Event-Id", id.to_string())
        .header("X-ValMed-Event", tipo)
        .header("X-ValMed-Timestamp", timestamp.to_string())
        .header(
            "X-ValMed-Signature",
            firma::firmar(secreto, timestamp, cuerpo),
        )
        .body(cuerpo.to_owned())
        .send()
        .await;

    let (estado, error, respondido) = match respuesta {
        Ok(r) => {
            let estado = r.status().as_u16();
            (Some(estado), None, Some(leer_inicio(r).await))
        }
        Err(e) => (None, Some(describir(&e)), None),
    };
    let texto = respondido.as_deref().unwrap_or_default();
    let clasificado = if api_market {
        clasificar_market(estado, texto)
    } else {
        clasificar(estado)
    };
    let error = match (&clasificado, error) {
        (Resultado::Reintentar, None) if api_market && matches!(estado, Some(200..=299)) => {
            Some("la respuesta no trae \"success\": true".to_owned())
        }
        (_, error) => error,
    };
    let intento = intentos_previos + 1;
    let resultado = match clasificado {
        Resultado::Reintentar if intento >= INTENTOS_MAXIMOS => Resultado::Fallido,
        otro => otro,
    };

    let (status, siguiente): (&str, Option<i64>) = match resultado {
        Resultado::Entregado => ("DELIVERED", None),
        Resultado::Fallido => ("FAILED", None),
        Resultado::Reintentar => ("PENDING", Some(ESPERAS_S[(intento - 1) as usize])),
    };
    tracing::info!(aviso = %id, tipo, intento, http = ?estado, status, "aviso enviado");

    sqlx::query(
        "UPDATE webhook_deliveries SET
             status = $2,
             attempts = $3,
             last_status_code = $4,
             last_error = $5,
             next_attempt_at = CASE WHEN $6::bigint IS NULL THEN next_attempt_at
                                    ELSE now() + make_interval(secs => $6::bigint) END,
             last_response = $7,
             delivered_at = CASE WHEN $2 = 'DELIVERED' THEN now() ELSE delivered_at END
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(intento)
    .bind(estado.map(i32::from))
    .bind(error)
    .bind(siguiente)
    .bind(respondido)
    .execute(banco.pool())
    .await?;
    Ok(())
}

fn describir(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout (5 s)".to_owned()
    } else if e.is_connect() {
        "no se pudo conectar".to_owned()
    } else {
        format!("error de red: {e}")
    }
}

#[derive(sqlx::FromRow)]
struct FilaAviso {
    id: Uuid,
    qr_charge_id: Uuid,
    event_type: String,
    format: String,
    url: String,
    status: String,
    attempts: i32,
    next_attempt_at: DateTime<Utc>,
    last_status_code: Option<i32>,
    last_error: Option<String>,
    last_response: Option<String>,
    created_at: DateTime<Utc>,
    delivered_at: Option<DateTime<Utc>>,
}

const SELECT_AVISO: &str = "
    SELECT d.id, d.qr_charge_id, d.event_type, d.format, d.url, d.status, d.attempts,
           d.next_attempt_at, d.last_status_code, d.last_error, d.last_response, d.created_at,
           d.delivered_at
    FROM webhook_deliveries d JOIN api_keys k ON k.id = d.api_key_id";

fn a_vista(f: FilaAviso) -> Aviso {
    Aviso {
        next_attempt_at: (f.status == "PENDING").then_some(f.next_attempt_at),
        id: f.id,
        qr_id: f.qr_charge_id,
        event_type: f.event_type,
        format: f.format,
        url: f.url,
        status: f.status,
        attempts: f.attempts,
        last_status_code: f.last_status_code,
        last_error: f.last_error,
        last_response: f.last_response,
        created_at: f.created_at,
        delivered_at: f.delivered_at,
    }
}

impl Banco {
    /// Avisos de un cliente (todas sus claves) o de una sola clave.
    pub async fn listar_avisos(
        &self,
        cliente_id: Uuid,
        clave_id: Option<Uuid>,
        limite: i64,
    ) -> Result<Vec<Aviso>, ErrorBanco> {
        let filas: Vec<FilaAviso> = sqlx::query_as(&format!(
            "{SELECT_AVISO}
             WHERE k.customer_id = $1 AND ($2::uuid IS NULL OR k.id = $2)
             ORDER BY d.created_at DESC
             LIMIT $3"
        ))
        .bind(cliente_id)
        .bind(clave_id)
        .bind(limite.clamp(1, 100))
        .fetch_all(self.pool())
        .await?;
        Ok(filas.into_iter().map(a_vista).collect())
    }

    /// Vuelve a enviar un aviso (aunque ya se haya entregado): sirve para probar
    /// que el integrador es idempotente.
    pub async fn reenviar_aviso(
        &self,
        cliente_id: Uuid,
        clave_id: Option<Uuid>,
        aviso_id: Uuid,
    ) -> Result<Aviso, ErrorBanco> {
        let afectadas = sqlx::query(
            "UPDATE webhook_deliveries d SET status = 'PENDING', attempts = 0, next_attempt_at = now()
             FROM api_keys k
             WHERE d.id = $1 AND k.id = d.api_key_id AND k.customer_id = $2
               AND ($3::uuid IS NULL OR k.id = $3)",
        )
        .bind(aviso_id)
        .bind(cliente_id)
        .bind(clave_id)
        .execute(self.pool())
        .await?
        .rows_affected();
        if afectadas == 0 {
            return Err(ErrorBanco::AvisoNoEncontrado);
        }
        let fila: FilaAviso = sqlx::query_as(&format!("{SELECT_AVISO} WHERE d.id = $1"))
            .bind(aviso_id)
            .fetch_one(self.pool())
            .await?;
        Ok(a_vista(fila))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clasifica_las_respuestas() {
        assert_eq!(clasificar(Some(200)), Resultado::Entregado);
        assert_eq!(clasificar(Some(204)), Resultado::Entregado);
        assert_eq!(clasificar(None), Resultado::Reintentar);
        assert_eq!(clasificar(Some(503)), Resultado::Reintentar);
        assert_eq!(clasificar(Some(429)), Resultado::Reintentar);
        assert_eq!(clasificar(Some(408)), Resultado::Reintentar);
        assert_eq!(clasificar(Some(401)), Resultado::Fallido);
        assert_eq!(clasificar(Some(400)), Resultado::Fallido);
        assert_eq!(clasificar(Some(302)), Resultado::Fallido);
    }

    #[test]
    fn el_formato_api_market_exige_success_true() {
        let ok = r#"{"success": true, "message": "OK"}"#;
        assert_eq!(clasificar_market(Some(200), ok), Resultado::Entregado);
        assert_eq!(
            clasificar_market(Some(200), r#"{"Success":true}"#),
            Resultado::Entregado
        );
        assert_eq!(
            clasificar_market(Some(200), r#"{"success": false, "message": "x"}"#),
            Resultado::Reintentar
        );
        assert_eq!(clasificar_market(Some(200), "OK"), Resultado::Reintentar);
        assert_eq!(clasificar_market(Some(503), ok), Resultado::Reintentar);
        assert_eq!(clasificar_market(Some(404), ok), Resultado::Fallido);
    }

    #[test]
    fn hay_siete_intentos_en_total() {
        assert_eq!(INTENTOS_MAXIMOS, 7);
        assert_eq!(ESPERAS_S.iter().sum::<i64>(), 4880);
    }
}
