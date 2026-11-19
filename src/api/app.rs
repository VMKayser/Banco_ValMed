//! API de la app web del banco (sesión con `Authorization: Bearer`).

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use rust_decimal::Decimal;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::extractores::{Consulta, Cuerpo, Ruta, Sesion, UrlBase};
use super::integracion::{motivo_devolucion, NuevaDevolucion};
use crate::dominio::{dinero, Ci};
use crate::error::{CuerpoError, ErrorBanco};
use crate::servicio::clientes::NuevoCliente;
use crate::servicio::cobros::DatosCobro;
use crate::servicio::vistas::{
    Aviso, ClaveApi, ClaveCreada, Cobro, Cuenta, Devolucion, EstadoPublico, Movimiento, Perfil,
    Recibo, SesionCreada,
};
use crate::servicio::Banco;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AbrirCuenta {
    #[schema(example = "7000005")]
    pub ci: String,
    #[schema(example = "1A")]
    pub ci_complement: Option<String>,
    #[schema(example = "Ana Torrez")]
    pub full_name: String,
    /// 6 dígitos.
    #[schema(example = "123456")]
    pub pin: String,
}

/// Abre una cuenta con el saldo inicial simulado y deja la sesión iniciada.
#[utoipa::path(
    post,
    path = "/api/v1/app/customers",
    tag = "app",
    request_body = AbrirCuenta,
    responses(
        (status = 201, body = SesionCreada),
        (status = 400, description = "VALIDATION_ERROR", body = CuerpoError),
        (status = 409, description = "CI_ALREADY_REGISTERED", body = CuerpoError),
    )
)]
pub async fn abrir_cuenta(
    State(banco): State<Banco>,
    Cuerpo(datos): Cuerpo<AbrirCuenta>,
) -> Result<(StatusCode, Json<SesionCreada>), ErrorBanco> {
    let ci = Ci::nuevo(&datos.ci, datos.ci_complement.as_deref())?;
    let cliente_id = banco
        .abrir_cuenta(NuevoCliente {
            ci: &ci,
            nombre: &datos.full_name,
            pin: &datos.pin,
            saldo_inicial: banco.config().saldo_inicial,
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(banco.crear_sesion(cliente_id).await?),
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IniciarSesion {
    #[schema(example = "7000001")]
    pub ci: String,
    pub ci_complement: Option<String>,
    #[schema(example = "123456")]
    pub pin: String,
}

/// Entrar con C.I. y PIN. Tras 5 PIN incorrectos seguidos, la cuenta se bloquea 5 min.
#[utoipa::path(
    post,
    path = "/api/v1/app/sessions",
    tag = "app",
    request_body = IniciarSesion,
    responses(
        (status = 201, body = SesionCreada),
        (status = 401, description = "INVALID_CREDENTIALS", body = CuerpoError),
        (status = 423, description = "ACCOUNT_LOCKED", body = CuerpoError),
    )
)]
pub async fn iniciar_sesion(
    State(banco): State<Banco>,
    Cuerpo(datos): Cuerpo<IniciarSesion>,
) -> Result<(StatusCode, Json<SesionCreada>), ErrorBanco> {
    let ci = Ci::nuevo(&datos.ci, datos.ci_complement.as_deref())
        .map_err(|_| ErrorBanco::CredencialesInvalidas)?;
    Ok((
        StatusCode::CREATED,
        Json(banco.iniciar_sesion(&ci, &datos.pin).await?),
    ))
}

/// Salir.
#[utoipa::path(
    delete,
    path = "/api/v1/app/sessions/current",
    tag = "app",
    security(("sesion" = [])),
    responses((status = 204), (status = 401, body = CuerpoError))
)]
pub async fn cerrar_sesion(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
) -> Result<StatusCode, ErrorBanco> {
    banco.cerrar_sesion(cliente.sesion_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Nombre, C.I., número de cuenta y saldo.
#[utoipa::path(
    get,
    path = "/api/v1/app/me",
    tag = "app",
    security(("sesion" = [])),
    responses((status = 200, body = Perfil), (status = 401, body = CuerpoError))
)]
pub async fn perfil(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
) -> Result<Json<Perfil>, ErrorBanco> {
    Ok(Json(banco.perfil(cliente.cliente_id).await?))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ParamsMovimientos {
    /// Cuántos (1 a 100, por defecto 20).
    pub limit: Option<i64>,
    /// `id` del último movimiento ya mostrado (para la página siguiente).
    pub before: Option<i64>,
}

/// Movimientos de la cuenta, del más reciente al más antiguo.
#[utoipa::path(
    get,
    path = "/api/v1/app/movements",
    tag = "app",
    security(("sesion" = [])),
    params(ParamsMovimientos),
    responses((status = 200, body = Vec<Movimiento>), (status = 401, body = CuerpoError))
)]
pub async fn movimientos(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Consulta(params): Consulta<ParamsMovimientos>,
) -> Result<Json<Vec<Movimiento>>, ErrorBanco> {
    Ok(Json(
        banco
            .movimientos(cliente.cuenta_id, params.limit.unwrap_or(20), params.before)
            .await?,
    ))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct Deposito {
    /// Máximo 5000 por operación.
    #[schema(value_type = String, example = "500.00")]
    pub amount: Decimal,
}

/// Depósito simulado: suma saldo a la cuenta.
#[utoipa::path(
    post,
    path = "/api/v1/app/deposits",
    tag = "app",
    security(("sesion" = [])),
    request_body = Deposito,
    responses((status = 200, body = Cuenta), (status = 400, body = CuerpoError))
)]
pub async fn depositar(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Cuerpo(datos): Cuerpo<Deposito>,
) -> Result<Json<Cuenta>, ErrorBanco> {
    let monto = dinero::monto("amount", datos.amount, dinero::maximo_deposito())?;
    Ok(Json(banco.depositar(cliente.cuenta_id, monto).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NuevoCobroApp {
    #[schema(value_type = String, example = "35.50")]
    pub amount: Decimal,
    #[schema(example = "Almuerzo")]
    pub description: String,
    /// De 30 a 86400; por defecto 600 (10 min).
    #[schema(example = 600)]
    pub expires_in_seconds: Option<i64>,
}

/// «Cobrar»: genera un QR dinámico que cobra a la cuenta de quien tiene la sesión.
#[utoipa::path(
    post,
    path = "/api/v1/app/qr",
    tag = "app",
    security(("sesion" = [])),
    request_body = NuevoCobroApp,
    responses((status = 201, body = Cobro), (status = 400, body = CuerpoError))
)]
pub async fn crear_cobro(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    UrlBase(base): UrlBase,
    Cuerpo(datos): Cuerpo<NuevoCobroApp>,
) -> Result<(StatusCode, Json<Cobro>), ErrorBanco> {
    let datos = DatosCobro::validar(
        None,
        datos.amount,
        None,
        &datos.description,
        datos.expires_in_seconds,
        None,
        None,
    )?;
    let (cobro, _) = banco
        .crear_cobro(cliente.cuenta_id, None, datos, &base)
        .await?;
    Ok((StatusCode::CREATED, Json(cobro)))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ParamsLista {
    /// Cuántos (1 a 100, por defecto 20).
    pub limit: Option<i64>,
}

/// Mis cobros, del más reciente al más antiguo.
#[utoipa::path(
    get,
    path = "/api/v1/app/qr",
    tag = "app",
    security(("sesion" = [])),
    params(ParamsLista),
    responses((status = 200, body = Vec<Cobro>))
)]
pub async fn listar_cobros(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    UrlBase(base): UrlBase,
    Consulta(params): Consulta<ParamsLista>,
) -> Result<Json<Vec<Cobro>>, ErrorBanco> {
    Ok(Json(
        banco
            .listar_cobros(cliente.cuenta_id, None, params.limit.unwrap_or(20), &base)
            .await?,
    ))
}

/// Un cobro mío, con el pagador si ya me pagaron.
#[utoipa::path(
    get,
    path = "/api/v1/app/qr/{id}",
    tag = "app",
    security(("sesion" = [])),
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses((status = 200, body = Cobro), (status = 404, body = CuerpoError))
)]
pub async fn cobro(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    UrlBase(base): UrlBase,
    Ruta(id): Ruta<Uuid>,
) -> Result<Json<Cobro>, ErrorBanco> {
    Ok(Json(
        banco.cobro_de_cuenta(id, cliente.cuenta_id, &base).await?,
    ))
}

/// Busca un cobro activo por su código corto (`7K2Q-M9XD`), para pagar sin cámara.
#[utoipa::path(
    get,
    path = "/api/v1/app/qr-codes/{code}",
    tag = "app",
    security(("sesion" = [])),
    params(("code" = String, Path, description = "Código del cobro, con o sin guion")),
    responses((status = 200, body = EstadoPublico), (status = 404, body = CuerpoError))
)]
pub async fn cobro_por_codigo(
    State(banco): State<Banco>,
    Sesion(_cliente): Sesion,
    Ruta(codigo): Ruta<String>,
) -> Result<Json<EstadoPublico>, ErrorBanco> {
    Ok(Json(banco.cobro_activo_por_codigo(&codigo).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PagarCobro {
    /// Se pide otra vez al confirmar, como en un banco real.
    #[schema(example = "123456")]
    pub pin: String,
}

/// Paga un cobro QR. Si esta cuenta ya lo pagó, responde 200 con el mismo recibo.
#[utoipa::path(
    post,
    path = "/api/v1/app/qr/{id}/payments",
    tag = "app",
    security(("sesion" = [])),
    params(("id" = Uuid, Path, description = "Id del cobro")),
    request_body = PagarCobro,
    responses(
        (status = 201, description = "Pagado", body = Recibo),
        (status = 200, description = "Ya lo habías pagado tú", body = Recibo),
        (status = 401, description = "INVALID_PIN", body = CuerpoError),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
        (status = 409, description = "INSUFFICIENT_FUNDS, QR_EXPIRED, QR_CANCELLED, QR_ALREADY_PAID o CANNOT_PAY_OWN_QR", body = CuerpoError),
        (status = 423, description = "ACCOUNT_LOCKED", body = CuerpoError),
    )
)]
pub async fn pagar(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    UrlBase(base): UrlBase,
    Ruta(id): Ruta<Uuid>,
    Cuerpo(datos): Cuerpo<PagarCobro>,
) -> Result<(StatusCode, Json<Recibo>), ErrorBanco> {
    let (recibo, nuevo) = banco
        .pagar(id, cliente.cliente_id, Some(&datos.pin), &base)
        .await?;
    let estado = if nuevo {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((estado, Json(recibo)))
}

/// Anula un cobro mío que nadie pagó.
#[utoipa::path(
    post,
    path = "/api/v1/app/qr/{id}/cancel",
    tag = "app",
    security(("sesion" = [])),
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses((status = 200, body = Cobro), (status = 409, body = CuerpoError))
)]
pub async fn anular(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    UrlBase(base): UrlBase,
    Ruta(id): Ruta<Uuid>,
) -> Result<Json<Cobro>, ErrorBanco> {
    Ok(Json(
        banco.anular_cobro(id, cliente.cuenta_id, &base).await?,
    ))
}

/// Devuelve un pago que recibí (completo).
#[utoipa::path(
    post,
    path = "/api/v1/app/payments/{bankReference}/refunds",
    tag = "app",
    security(("sesion" = [])),
    params(("bankReference" = String, Path, description = "Referencia del pago, ej. VM-20261118-000042")),
    request_body = NuevaDevolucion,
    responses(
        (status = 201, body = Devolucion),
        (status = 200, description = "Ya estaba devuelto", body = Devolucion),
        (status = 404, description = "PAYMENT_NOT_FOUND", body = CuerpoError),
        (status = 409, description = "INSUFFICIENT_FUNDS", body = CuerpoError),
    )
)]
pub async fn devolver(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    UrlBase(base): UrlBase,
    Ruta(referencia): Ruta<String>,
    Cuerpo(datos): Cuerpo<NuevaDevolucion>,
) -> Result<(StatusCode, Json<Devolucion>), ErrorBanco> {
    let motivo = motivo_devolucion(datos.reason.as_deref())?;
    let (devolucion, nueva) = banco
        .devolver(&referencia, cliente.cuenta_id, motivo, &base)
        .await?;
    let estado = if nueva {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((estado, Json(devolucion)))
}

/// Mis claves de API (sin la clave en sí, que se muestra una sola vez).
#[utoipa::path(
    get,
    path = "/api/v1/app/api-keys",
    tag = "app",
    security(("sesion" = [])),
    responses((status = 200, body = Vec<ClaveApi>))
)]
pub async fn listar_claves(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
) -> Result<Json<Vec<ClaveApi>>, ErrorBanco> {
    Ok(Json(banco.listar_claves(cliente.cliente_id).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NuevaClave {
    #[schema(example = "SmartFila Ventas")]
    pub name: String,
    /// Opcional: adónde mandar los avisos firmados.
    #[schema(example = "http://sales:8080/api/v1/ventas/pagos/notificaciones")]
    pub webhook_url: Option<String>,
}

/// Crea una clave de API. La clave y el secreto de avisos se muestran solo aquí.
#[utoipa::path(
    post,
    path = "/api/v1/app/api-keys",
    tag = "app",
    security(("sesion" = [])),
    request_body = NuevaClave,
    responses(
        (status = 201, body = ClaveCreada),
        (status = 409, description = "API_KEY_LIMIT", body = CuerpoError),
    )
)]
pub async fn crear_clave(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Cuerpo(datos): Cuerpo<NuevaClave>,
) -> Result<(StatusCode, Json<ClaveCreada>), ErrorBanco> {
    let creada = banco
        .crear_clave(cliente.cliente_id, &datos.name, datos.webhook_url)
        .await?;
    Ok((StatusCode::CREATED, Json(creada)))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CambiarWebhook {
    /// `null` o vacío para dejar de recibir avisos.
    pub webhook_url: Option<String>,
}

/// Cambia (o quita) la URL de avisos de una clave.
#[utoipa::path(
    patch,
    path = "/api/v1/app/api-keys/{id}",
    tag = "app",
    security(("sesion" = [])),
    params(("id" = Uuid, Path, description = "Id de la clave")),
    request_body = CambiarWebhook,
    responses((status = 200, body = ClaveApi), (status = 404, body = CuerpoError))
)]
pub async fn cambiar_webhook(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Ruta(id): Ruta<Uuid>,
    Cuerpo(datos): Cuerpo<CambiarWebhook>,
) -> Result<Json<ClaveApi>, ErrorBanco> {
    Ok(Json(
        banco
            .cambiar_webhook(cliente.cliente_id, id, datos.webhook_url)
            .await?,
    ))
}

/// Revoca una clave: deja de servir de inmediato.
#[utoipa::path(
    delete,
    path = "/api/v1/app/api-keys/{id}",
    tag = "app",
    security(("sesion" = [])),
    params(("id" = Uuid, Path, description = "Id de la clave")),
    responses((status = 204), (status = 404, body = CuerpoError))
)]
pub async fn revocar_clave(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Ruta(id): Ruta<Uuid>,
) -> Result<StatusCode, ErrorBanco> {
    banco.revocar_clave(cliente.cliente_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Avisos enviados a los webhooks de mis claves.
#[utoipa::path(
    get,
    path = "/api/v1/app/webhook-deliveries",
    tag = "app",
    security(("sesion" = [])),
    params(ParamsLista),
    responses((status = 200, body = Vec<Aviso>))
)]
pub async fn listar_avisos(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Consulta(params): Consulta<ParamsLista>,
) -> Result<Json<Vec<Aviso>>, ErrorBanco> {
    Ok(Json(
        banco
            .listar_avisos(cliente.cliente_id, None, params.limit.unwrap_or(20))
            .await?,
    ))
}

/// Vuelve a enviar un aviso.
#[utoipa::path(
    post,
    path = "/api/v1/app/webhook-deliveries/{id}/redeliver",
    tag = "app",
    security(("sesion" = [])),
    params(("id" = Uuid, Path, description = "Id del aviso")),
    responses((status = 202, body = Aviso), (status = 404, body = CuerpoError))
)]
pub async fn reenviar_aviso(
    State(banco): State<Banco>,
    Sesion(cliente): Sesion,
    Ruta(id): Ruta<Uuid>,
) -> Result<(StatusCode, Json<Aviso>), ErrorBanco> {
    Ok((
        StatusCode::ACCEPTED,
        Json(banco.reenviar_aviso(cliente.cliente_id, None, id).await?),
    ))
}
