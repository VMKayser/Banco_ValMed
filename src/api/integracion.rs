//! API para integradores (otros sistemas que cobran con QR), con `X-Api-Key`.
//! Se llama desde el backend del integrador, nunca desde un navegador.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::extractores::{ClaveOToken, ConClave, Consulta, Cuerpo, Ruta, UrlBase};
use crate::dominio::Ci;
use crate::error::{CuerpoError, ErrorBanco};
use crate::infra::qr;
use crate::servicio::cobros::{DatosCobro, MOTIVOS_DEVOLUCION};
use crate::servicio::semilla::asegurar_cliente;
use crate::servicio::vistas::{Aviso, Cobro, Devolucion, Recibo};
use crate::servicio::Banco;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NuevoCobro {
    /// Tu referencia (única por cuenta). Repetirla con los mismos datos devuelve
    /// el cobro existente; con otros datos, 409.
    #[schema(example = "orden-6f1c2a9e-intento-1")]
    pub merchant_reference: Option<String>,
    /// De 0.01 a 100000, máximo 2 decimales. Mejor como string.
    #[schema(value_type = String, example = "350.00")]
    pub amount: Decimal,
    /// Solo `BOB` (opcional).
    #[schema(example = "BOB")]
    pub currency: Option<String>,
    /// Lo que verá quien paga (1 a 140 caracteres).
    #[schema(example = "Entrada Guadalupe · Tramo A-014")]
    pub description: String,
    /// De 30 a 86400 s. Por defecto 600. No se puede usar junto con `expiresAt`.
    #[schema(example = 600)]
    pub expires_in_seconds: Option<i64>,
    /// Fecha exacta de vencimiento (entre 30 s y 24 h desde ahora).
    pub expires_at: Option<DateTime<Utc>>,
    /// Objeto JSON libre (hasta 2000 bytes); vuelve tal cual en consultas y avisos.
    #[schema(value_type = Option<Object>, example = json!({"ordenId": "6f1c2a9e"}))]
    pub metadata: Option<Value>,
}

/// Crea un cobro QR dinámico. La respuesta trae el texto del QR (`payload`), la
/// imagen en base64 y los enlaces públicos para mostrarlo y seguir su estado.
#[utoipa::path(
    post,
    path = "/api/v1/qr",
    tag = "integración",
    security(("apiKey" = [])),
    request_body = NuevoCobro,
    responses(
        (status = 201, description = "Cobro creado", body = Cobro),
        (status = 200, description = "Ya existía con esa merchantReference y los mismos datos", body = Cobro),
        (status = 400, description = "VALIDATION_ERROR", body = CuerpoError),
        (status = 401, description = "UNAUTHENTICATED", body = CuerpoError),
        (status = 409, description = "MERCHANT_REFERENCE_CONFLICT", body = CuerpoError),
    )
)]
pub async fn crear_cobro(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    UrlBase(base): UrlBase,
    Cuerpo(datos): Cuerpo<NuevoCobro>,
) -> Result<(StatusCode, Json<Cobro>), ErrorBanco> {
    let datos = DatosCobro::validar(
        datos.merchant_reference,
        datos.amount,
        datos.currency.as_deref(),
        &datos.description,
        datos.expires_in_seconds,
        datos.expires_at,
        datos.metadata,
    )?;
    let (mut cobro, nuevo) = banco
        .crear_cobro(
            integrador.cuenta_id,
            Some(integrador.clave_id),
            datos,
            &base,
        )
        .await?;
    cobro.image_png_base64 = Some(STANDARD.encode(qr::png(&cobro.payload)?));
    let estado = if nuevo {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((estado, Json(cobro)))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct ParamsCobros {
    /// Filtra por tu referencia.
    pub merchant_reference: Option<String>,
    /// Cuántos (1 a 100, por defecto 20).
    pub limit: Option<i64>,
}

/// Lista tus cobros (todos los de la cuenta dueña de la clave), del más reciente
/// al más antiguo.
#[utoipa::path(
    get,
    path = "/api/v1/qr",
    tag = "integración",
    security(("apiKey" = [])),
    params(ParamsCobros),
    responses((status = 200, body = Vec<Cobro>), (status = 401, body = CuerpoError))
)]
pub async fn listar_cobros(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    UrlBase(base): UrlBase,
    Consulta(params): Consulta<ParamsCobros>,
) -> Result<Json<Vec<Cobro>>, ErrorBanco> {
    Ok(Json(
        banco
            .listar_cobros(
                integrador.cuenta_id,
                params.merchant_reference.as_deref(),
                params.limit.unwrap_or(20),
                &base,
            )
            .await?,
    ))
}

/// Consulta un cobro: estado y, si ya se pagó, quién pagó (C.I., nombre, cuenta).
#[utoipa::path(
    get,
    path = "/api/v1/qr/{id}",
    tag = "integración",
    security(("apiKey" = [])),
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses(
        (status = 200, body = Cobro),
        (status = 401, body = CuerpoError),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
    )
)]
pub async fn cobro(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    UrlBase(base): UrlBase,
    Ruta(id): Ruta<Uuid>,
) -> Result<Json<Cobro>, ErrorBanco> {
    Ok(Json(
        banco
            .cobro_de_cuenta(id, integrador.cuenta_id, &base)
            .await?,
    ))
}

/// Anula un cobro que nadie pagó. Anular uno ya anulado no es error.
#[utoipa::path(
    post,
    path = "/api/v1/qr/{id}/cancel",
    tag = "integración",
    security(("apiKey" = [])),
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses(
        (status = 200, body = Cobro),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
        (status = 409, description = "QR_ALREADY_PAID o QR_EXPIRED", body = CuerpoError),
    )
)]
pub async fn anular(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    UrlBase(base): UrlBase,
    Ruta(id): Ruta<Uuid>,
) -> Result<Json<Cobro>, ErrorBanco> {
    Ok(Json(
        banco.anular_cobro(id, integrador.cuenta_id, &base).await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NuevaDevolucion {
    /// `PAGADOR_DISTINTO`, `PAGO_TARDIO`, `MONTO_DISTINTO` o `DEVOLUCION` (por defecto).
    #[schema(example = "PAGADOR_DISTINTO")]
    pub reason: Option<String>,
}

pub(crate) fn motivo_devolucion(motivo: Option<&str>) -> Result<&'static str, ErrorBanco> {
    let motivo = motivo
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("DEVOLUCION");
    MOTIVOS_DEVOLUCION
        .iter()
        .find(|m| m.eq_ignore_ascii_case(motivo))
        .copied()
        .ok_or_else(|| ErrorBanco::validacion("reason", MOTIVOS_DEVOLUCION.join(", ")))
}

/// Devuelve un pago completo a quien pagó. Repetirla responde la misma devolución.
#[utoipa::path(
    post,
    path = "/api/v1/payments/{bankReference}/refunds",
    tag = "integración",
    security(("apiKey" = [])),
    params(("bankReference" = String, Path, description = "Referencia del pago, ej. VM-20261118-000042")),
    request_body = NuevaDevolucion,
    responses(
        (status = 201, body = Devolucion),
        (status = 200, description = "Ya estaba devuelto", body = Devolucion),
        (status = 404, description = "PAYMENT_NOT_FOUND", body = CuerpoError),
        (status = 409, description = "INSUFFICIENT_FUNDS (tu cuenta no tiene saldo para devolver)", body = CuerpoError),
    )
)]
pub async fn devolver(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    UrlBase(base): UrlBase,
    Ruta(referencia): Ruta<String>,
    Cuerpo(datos): Cuerpo<NuevaDevolucion>,
) -> Result<(StatusCode, Json<Devolucion>), ErrorBanco> {
    let motivo = motivo_devolucion(datos.reason.as_deref())?;
    let (devolucion, nueva) = banco
        .devolver(&referencia, integrador.cuenta_id, motivo, &base)
        .await?;
    let estado = if nueva {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((estado, Json(devolucion)))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ParamsAvisos {
    /// Cuántos (1 a 100, por defecto 20).
    pub limit: Option<i64>,
}

/// Avisos enviados al webhook de esta clave, con intentos y última respuesta.
#[utoipa::path(
    get,
    path = "/api/v1/webhook-deliveries",
    tag = "integración",
    security(("apiKey" = [])),
    params(ParamsAvisos),
    responses((status = 200, body = Vec<Aviso>))
)]
pub async fn listar_avisos(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    Consulta(params): Consulta<ParamsAvisos>,
) -> Result<Json<Vec<Aviso>>, ErrorBanco> {
    Ok(Json(
        banco
            .listar_avisos(
                integrador.cliente_id,
                Some(integrador.clave_id),
                params.limit.unwrap_or(20),
            )
            .await?,
    ))
}

/// Vuelve a enviar un aviso (mismo id y cuerpo): sirve para probar que tu
/// receptor es idempotente.
#[utoipa::path(
    post,
    path = "/api/v1/webhook-deliveries/{id}/redeliver",
    tag = "integración",
    security(("apiKey" = [])),
    params(("id" = Uuid, Path, description = "Id del aviso")),
    responses((status = 202, body = Aviso), (status = 404, body = CuerpoError))
)]
pub async fn reenviar_aviso(
    State(banco): State<Banco>,
    ConClave(integrador): ConClave,
    Ruta(id): Ruta<Uuid>,
) -> Result<(StatusCode, Json<Aviso>), ErrorBanco> {
    Ok((
        StatusCode::ACCEPTED,
        Json(
            banco
                .reenviar_aviso(integrador.cliente_id, Some(integrador.clave_id), id)
                .await?,
        ),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PagoSandbox {
    /// C.I. de quien paga (`1234567` o `1234567-1A`). Si no tiene cuenta, se le
    /// abre una con el saldo inicial y PIN 123456.
    #[schema(example = "7000001")]
    pub payer_ci: String,
}

/// Sandbox: paga un cobro sin la app ni PIN, desde la cuenta de `payerCi`. Mismas
/// reglas y mismo aviso que un pago real. Solo con `BANK_SANDBOX=true`. Acepta
/// `X-Api-Key` o el token de la API Market. No existe en el BNB.
#[utoipa::path(
    post,
    path = "/api/v1/sandbox/qr/{id}/pay",
    tag = "sandbox",
    security(("apiKey" = []), ("tokenMarket" = [])),
    params(("id" = String, Path, description = "Id del cobro (UUID) o `qrId` numérico de la API Market")),
    request_body = PagoSandbox,
    responses(
        (status = 201, body = Recibo),
        (status = 200, description = "Esa cuenta ya lo había pagado", body = Recibo),
        (status = 404, description = "NOT_FOUND (sandbox apagado) o QR_NOT_FOUND", body = CuerpoError),
        (status = 409, description = "INSUFFICIENT_FUNDS, QR_EXPIRED, QR_CANCELLED, QR_ALREADY_PAID o CANNOT_PAY_OWN_QR", body = CuerpoError),
    )
)]
pub async fn pagar_sandbox(
    State(banco): State<Banco>,
    ClaveOToken(_integrador): ClaveOToken,
    UrlBase(base): UrlBase,
    Ruta(id): Ruta<String>,
    Cuerpo(datos): Cuerpo<PagoSandbox>,
) -> Result<(StatusCode, Json<Recibo>), ErrorBanco> {
    if !banco.config().sandbox {
        return Err(ErrorBanco::NoEncontrado);
    }
    let id = match (Uuid::parse_str(&id), id.parse::<i64>()) {
        (Ok(uuid), _) => uuid,
        (_, Ok(numero)) => banco
            .id_por_numero(numero)
            .await?
            .ok_or(ErrorBanco::QrNoEncontrado)?,
        _ => return Err(ErrorBanco::QrNoEncontrado),
    };
    let ci = Ci::desde_texto(&datos.payer_ci)?;
    let cliente_id = asegurar_cliente(
        &banco,
        &ci,
        &format!("Cliente sandbox {ci}"),
        banco.config().saldo_inicial,
    )
    .await?;
    let (recibo, nuevo) = banco.pagar(id, cliente_id, None, &base).await?;
    let estado = if nuevo {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((estado, Json(recibo)))
}
