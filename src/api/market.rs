//! API Market (Pago QR) compatible con la del Banco Nacional de Bolivia: mismas
//! rutas, mismos campos y respuestas `{ success, message }`. Pasar al BNB es
//! cambiar la URL base y las credenciales.
//!
//! Como el BNB: un rechazo de negocio responde HTTP 200 con `success: false`;
//! hay que mirar siempre `success`. Un cuerpo mal formado responde 400 y un
//! token inválido o vencido, 401.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use super::extractores::UrlBase;
use crate::dominio::market::{self, Campos};
use crate::error::{capitalizar, ErrorBanco};
use crate::infra::qr;
use crate::servicio::claves::Integrador;
use crate::servicio::cobros::{DatosCobro, FilaCobro};
use crate::servicio::market::EstadoNotificacion;
use crate::servicio::Banco;

// ---------------------------------------------------------------------------
// Respuestas y errores
// ---------------------------------------------------------------------------

/// Respuesta base del BNB: `success` dice si la operación se hizo; si no,
/// `message` explica por qué.
#[derive(Debug, Serialize, ToSchema)]
pub struct Respuesta {
    pub success: bool,
    /// Token (en `auth/token`), motivo del rechazo o `null`.
    #[schema(example = "QR no encontrado")]
    pub message: Option<String>,
}

#[derive(Debug)]
pub struct ErrorMarket {
    estado: StatusCode,
    mensaje: String,
}

impl ErrorMarket {
    /// Cuerpo mal formado o sin un campo obligatorio: 400.
    fn cuerpo(mensaje: impl Into<String>) -> Self {
        Self {
            estado: StatusCode::BAD_REQUEST,
            mensaje: mensaje.into(),
        }
    }

    /// La operación no se puede hacer: 200 con `success: false`, como el BNB.
    fn rechazo(mensaje: impl Into<String>) -> Self {
        Self {
            estado: StatusCode::OK,
            mensaje: mensaje.into(),
        }
    }

    fn token() -> Self {
        Self {
            estado: StatusCode::UNAUTHORIZED,
            mensaje:
                "Token inválido o vencido: pide otro en /ClientAuthentication.API/api/v1/auth/token"
                    .to_owned(),
        }
    }
}

impl From<ErrorBanco> for ErrorMarket {
    fn from(error: ErrorBanco) -> Self {
        let (estado, _) = error.estado_y_codigo();
        match error {
            ErrorBanco::Validacion(detalles) => Self::rechazo(
                detalles
                    .iter()
                    .map(|d| format!("{}: {}", d.field, d.issue))
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            ErrorBanco::NoAutenticado(_) => Self::token(),
            _ if estado.is_server_error() => {
                tracing::error!(error = %error, "error en la API Market");
                Self {
                    estado,
                    mensaje: if estado == StatusCode::SERVICE_UNAVAILABLE {
                        "Servicio temporalmente no disponible".to_owned()
                    } else {
                        "Error interno del banco".to_owned()
                    },
                }
            }
            otro => Self::rechazo(capitalizar(&otro.to_string())),
        }
    }
}

impl IntoResponse for ErrorMarket {
    fn into_response(self) -> Response {
        let token_invalido = self.estado == StatusCode::UNAUTHORIZED;
        let mut respuesta = (
            self.estado,
            Json(Respuesta {
                success: false,
                message: Some(self.mensaje),
            }),
        )
            .into_response();
        if token_invalido {
            respuesta
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        respuesta
    }
}

/// Rutas que no existen bajo `/QRSimple.API` o `/ClientAuthentication.API`.
pub async fn no_encontrado() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(Respuesta {
            success: false,
            message: Some("Servicio no encontrado".to_owned()),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Extractores
// ---------------------------------------------------------------------------

/// Cuerpo JSON (objeto) con las claves en minúsculas.
pub struct CuerpoMarket(pub Campos);

impl<S: Send + Sync> FromRequest<S> for CuerpoMarket {
    type Rejection = ErrorMarket;

    async fn from_request(peticion: Request, estado: &S) -> Result<Self, ErrorMarket> {
        let Json(valor) = Json::<Value>::from_request(peticion, estado)
            .await
            .map_err(|r| match r {
                JsonRejection::MissingJsonContentType(_) => {
                    ErrorMarket::cuerpo("Falta la cabecera Content-Type: application/json")
                }
                otro => ErrorMarket::cuerpo(format!(
                    "El cuerpo no es un JSON válido: {}",
                    otro.body_text()
                )),
            })?;
        Campos::desde_json(valor)
            .map(Self)
            .ok_or_else(|| ErrorMarket::cuerpo("El cuerpo debe ser un objeto JSON"))
    }
}

/// `Authorization: Bearer <token de auth/token>`.
pub struct TokenMarket(pub Integrador);

impl FromRequestParts<Banco> for TokenMarket {
    type Rejection = ErrorMarket;

    async fn from_request_parts(parts: &mut Parts, banco: &Banco) -> Result<Self, ErrorMarket> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .filter(|(esquema, _)| esquema.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim())
            .filter(|t| !t.is_empty())
            .ok_or_else(ErrorMarket::token)?;
        banco
            .integrador_market(token)
            .await?
            .map(TokenMarket)
            .ok_or_else(ErrorMarket::token)
    }
}

// ---------------------------------------------------------------------------
// Esquemas de las peticiones (solo para la documentación: el cuerpo se lee con
// `Campos`, que acepta números como texto y claves en cualquier mayúscula)
// ---------------------------------------------------------------------------

#[allow(dead_code, non_snake_case)]
#[derive(ToSchema)]
pub struct PedidoToken {
    /// Identificador de la clave (público). Está en la app, «Desarrolladores».
    #[schema(example = "s9CG8FE7Id75ef2jeX9bUA==")]
    accountId: String,
    /// Secreto: al crear la clave es la misma clave de API (`vm_test_…`).
    #[schema(example = "vm_test_Xy12ab…")]
    authorizationId: String,
}

#[allow(dead_code, non_snake_case)]
#[derive(ToSchema)]
pub struct PedidoCredenciales {
    #[schema(example = "s9CG8FE7Id75ef2jeX9bUA==")]
    AccountId: String,
    #[schema(example = "vm_test_Xy12ab…")]
    actualAuthorizationId: String,
    /// 15 caracteres o más, con al menos una letra, un número y un carácter
    /// especial (`+ - / * $ < > ! ¡ # | =`).
    #[schema(example = "Naciona0123456*")]
    newAuthorizationId: String,
}

#[allow(dead_code, non_snake_case)]
#[derive(ToSchema)]
pub struct PedidoQr {
    /// Solo `BOB`.
    #[schema(example = "BOB")]
    currency: String,
    /// Descripción del pago (1 a 140 caracteres). Vuelve en la notificación como `Gloss`.
    #[schema(example = "Entrada A-014")]
    gloss: String,
    /// Monto (número o texto), de 0.01 a 100000 con 2 decimales como máximo.
    #[schema(example = 350)]
    amount: f64,
    /// Solo `true` (QR de un solo pago).
    #[schema(example = true)]
    singleUse: bool,
    /// Último día en que se puede pagar (`AAAA-MM-DD`, hora de Bolivia), desde
    /// hoy hasta 365 días. Vence a las 00:00 del día siguiente.
    #[schema(example = "2026-11-18")]
    expirationDate: String,
    /// Opcional, hasta 500 caracteres. Vuelve en la notificación.
    #[schema(example = "orden-6f1c2a9e")]
    additionalData: Option<String>,
    /// Opcional: `1` (cuenta en bolivianos, la única que existe).
    #[schema(example = "1")]
    destinationAccountId: Option<String>,
}

#[allow(dead_code, non_snake_case)]
#[derive(ToSchema)]
pub struct PedidoQrId {
    /// El `id` que devolvió `getQRWithImageAsync` (número o texto).
    #[schema(example = 51)]
    qrId: i64,
}

#[allow(dead_code, non_snake_case)]
#[derive(ToSchema)]
pub struct PedidoFecha {
    /// Día de creación (`AAAA-MM-DD`, hora de Bolivia).
    #[schema(example = "2026-11-18")]
    generationDate: String,
}

// ---------------------------------------------------------------------------
// Respuestas con datos
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub struct QrGenerado {
    /// Id del QR, como texto (así lo devuelve el BNB).
    #[schema(example = "51")]
    pub id: String,
    /// Imagen PNG del QR en base64: `<img src="data:image/png;base64,…">`.
    pub qr: String,
    pub success: bool,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EstadoQr {
    #[schema(example = 51)]
    pub id: i64,
    /// 1 = no usado, 2 = usado (pagado), 3 = expirado, 4 = con error (aquí: anulado).
    #[schema(example = 2)]
    pub status_id: i32,
    /// Hora de Bolivia, sin zona.
    #[schema(example = "2026-11-19T00:00:00")]
    pub expiration_date: String,
    /// Código de transacción si se pagó; si no, `null`.
    #[schema(example = "3JAO702813")]
    pub voucher_id: Option<String>,
    pub success: bool,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DetalleQr {
    #[schema(example = 49)]
    pub id: i64,
    #[schema(value_type = f64, example = 20)]
    pub amount: Value,
    #[schema(example = "BOB")]
    pub currency: String,
    #[schema(example = "2026-11-18T19:44:01")]
    pub generation_date: String,
    #[schema(example = "2026-11-19T00:00:00")]
    pub expiration_date: String,
    /// 1 = no usado, 2 = usado, 3 = expirado, 4 = con error (aquí: anulado).
    #[schema(example = 1)]
    pub status_id: i32,
    /// Cuándo se pagó; `0001-01-01T00:00:00` si no se usó.
    #[schema(example = "0001-01-01T00:00:00")]
    pub transaction_date: String,
    /// Banco desde el que se pagó; `null` si no se usó.
    #[schema(example = "BANCO VALMED")]
    pub source_bank: Option<String>,
    #[schema(example = "Entrada A-014")]
    pub gloss: String,
    #[schema(example = 1)]
    pub destination_account_number: i64,
    /// Si tu servidor respondió `{"success": true}` a la notificación de pago.
    pub notification_success: bool,
    /// Lo que respondió tu servidor a la notificación.
    #[schema(example = r#"{"success":true,"message":"OK"}"#)]
    pub notification_response: Option<String>,
    #[schema(example = "3JAO702813")]
    pub voucher_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct QrsDelDia {
    /// Así se llama en el BNB (con esa mezcla de mayúsculas).
    #[serde(rename = "dTOqrDetails")]
    pub detalles: Vec<DetalleQr>,
    pub success: bool,
    pub message: Option<String>,
}

fn detalle(fila: FilaCobro, aviso: Option<EstadoNotificacion>) -> DetalleQr {
    let (notificado, respuesta) = aviso.unwrap_or((false, None));
    DetalleQr {
        id: fila.number,
        amount: market::numero(fila.amount),
        status_id: market::estado_id(fila.estado()),
        generation_date: market::fecha_local(fila.created_at),
        expiration_date: market::fecha_local(fila.expires_at),
        transaction_date: fila
            .paid_at
            .map(market::fecha_local)
            .unwrap_or_else(|| market::FECHA_SIN_USO.to_owned()),
        source_bank: fila.paid_at.map(|_| market::NOMBRE_BANCO.to_owned()),
        destination_account_number: market::CUENTA_DESTINO_BOB,
        notification_success: notificado,
        notification_response: respuesta,
        voucher_id: fila.voucher_id,
        currency: fila.currency,
        gloss: fila.description,
    }
}

fn qr_id(campos: &Campos) -> Result<i64, ErrorMarket> {
    campos
        .entero("qrId")
        .map_err(ErrorMarket::cuerpo)?
        .ok_or_else(|| ErrorMarket::cuerpo("qrId es obligatorio"))
}

// ---------------------------------------------------------------------------
// Servicios
// ---------------------------------------------------------------------------

/// Genera el token para los demás servicios (va en `Authorization: Bearer`).
/// Dura 1 hora; al vencer, los servicios responden 401 y hay que pedir otro.
#[utoipa::path(
    post,
    path = "/ClientAuthentication.API/api/v1/auth/token",
    tag = "api-market",
    request_body = PedidoToken,
    responses(
        (status = 200, description = "`success: true` y el token en `message`; `success: false` si las credenciales no sirven", body = Respuesta),
        (status = 400, description = "Cuerpo mal formado", body = Respuesta),
    )
)]
pub async fn token(
    State(banco): State<Banco>,
    CuerpoMarket(campos): CuerpoMarket,
) -> Result<Json<Respuesta>, ErrorMarket> {
    let account_id = campos.texto("accountId").map_err(ErrorMarket::cuerpo)?;
    let autorizacion = campos
        .texto("authorizationId")
        .map_err(ErrorMarket::cuerpo)?;
    match banco.token_market(&account_id, &autorizacion).await? {
        Some(token) => Ok(Json(Respuesta {
            success: true,
            message: Some(token),
        })),
        None => Err(ErrorMarket::rechazo("Credenciales inválidas")),
    }
}

/// Cambia el `authorizationId`. El BNB obliga a hacerlo antes del primer uso;
/// aquí es opcional. `X-Api-Key` no cambia.
#[utoipa::path(
    post,
    path = "/ClientAuthentication.API/api/v1/auth/UpdateCredentials",
    tag = "api-market",
    request_body = PedidoCredenciales,
    responses(
        (status = 200, description = "`success: true` (message vacío) o `success: false` con el motivo", body = Respuesta),
        (status = 400, description = "Cuerpo mal formado", body = Respuesta),
    )
)]
pub async fn cambiar_credenciales(
    State(banco): State<Banco>,
    CuerpoMarket(campos): CuerpoMarket,
) -> Result<Json<Respuesta>, ErrorMarket> {
    let account_id = campos.texto("accountId").map_err(ErrorMarket::cuerpo)?;
    let actual = campos
        .texto("actualAuthorizationId")
        .map_err(ErrorMarket::cuerpo)?;
    let nueva = campos
        .texto("newAuthorizationId")
        .map_err(ErrorMarket::cuerpo)?;
    if !market::autorizacion_segura(&nueva) {
        return Err(ErrorMarket::rechazo(
            "newAuthorizationId debe tener al menos 15 caracteres, una letra, un número y un carácter especial (+ - / * $ < > ! ¡ # | =)",
        ));
    }
    if nueva == actual {
        return Err(ErrorMarket::rechazo(
            "newAuthorizationId debe ser distinta de la actual",
        ));
    }
    if !banco
        .cambiar_autorizacion_market(&account_id, &actual, &nueva)
        .await?
    {
        return Err(ErrorMarket::rechazo(
            "accountId o actualAuthorizationId incorrectos",
        ));
    }
    Ok(Json(Respuesta {
        success: true,
        message: Some(String::new()),
    }))
}

/// Genera un QR de cobro y devuelve su imagen (PNG en base64). Cuando alguien
/// lo paga, el banco llama a tu URL de notificación (`ReceiveNotification`).
#[utoipa::path(
    post,
    path = "/QRSimple.API/api/v1/main/getQRWithImageAsync",
    tag = "api-market",
    security(("tokenMarket" = [])),
    request_body = PedidoQr,
    responses(
        (status = 200, description = "QR creado, o `success: false` con el motivo", body = QrGenerado),
        (status = 400, description = "Cuerpo mal formado", body = Respuesta),
        (status = 401, description = "Token inválido o vencido", body = Respuesta),
    )
)]
pub async fn generar_qr(
    State(banco): State<Banco>,
    TokenMarket(integrador): TokenMarket,
    UrlBase(base): UrlBase,
    CuerpoMarket(campos): CuerpoMarket,
) -> Result<Json<QrGenerado>, ErrorMarket> {
    let moneda = campos.texto("currency").map_err(ErrorMarket::cuerpo)?;
    let glosa = campos.texto("gloss").map_err(ErrorMarket::cuerpo)?;
    let monto = campos.decimal("amount").map_err(ErrorMarket::cuerpo)?;
    let uso_unico = campos
        .booleano("singleUse")
        .map_err(ErrorMarket::cuerpo)?
        .ok_or_else(|| ErrorMarket::cuerpo("singleUse es obligatorio"))?;
    let ultimo_dia = campos
        .fecha("expirationDate")
        .map_err(ErrorMarket::cuerpo)?;
    let adicional = campos
        .texto_opcional("additionalData")
        .map_err(ErrorMarket::cuerpo)?;
    let cuenta_destino = campos
        .entero("destinationAccountId")
        .map_err(ErrorMarket::cuerpo)?;

    if !moneda.eq_ignore_ascii_case("BOB") {
        return Err(ErrorMarket::rechazo(
            "Banco ValMed solo cobra en bolivianos: currency debe ser BOB",
        ));
    }
    if !uso_unico {
        return Err(ErrorMarket::rechazo(
            "Banco ValMed solo emite QR de un solo pago: singleUse debe ser true",
        ));
    }
    if cuenta_destino.is_some_and(|c| c != market::CUENTA_DESTINO_BOB) {
        return Err(ErrorMarket::rechazo(
            "destinationAccountId debe ser 1 (cuenta en bolivianos): Banco ValMed no tiene cuentas en moneda extranjera",
        ));
    }
    let vence = market::vencimiento(ultimo_dia, Utc::now()).map_err(ErrorMarket::rechazo)?;

    let datos = DatosCobro::api_market(monto, &glosa, vence, adicional)?;
    let (cobro, _) = banco
        .crear_cobro(
            integrador.cuenta_id,
            Some(integrador.clave_id),
            datos,
            &base,
        )
        .await?;
    let png = qr::png(&cobro.payload).map_err(ErrorBanco::from)?;
    Ok(Json(QrGenerado {
        id: cobro.number.to_string(),
        qr: STANDARD.encode(png),
        success: true,
        message: None,
    }))
}

/// QR generados por tu cuenta en un día (hora de Bolivia), con su estado y el
/// resultado de la notificación de pago.
#[utoipa::path(
    post,
    path = "/QRSimple.API/api/v1/main/getQRbyGenerationDateAsync",
    tag = "api-market",
    security(("tokenMarket" = [])),
    request_body = PedidoFecha,
    responses(
        (status = 200, body = QrsDelDia),
        (status = 400, description = "Cuerpo mal formado", body = Respuesta),
        (status = 401, description = "Token inválido o vencido", body = Respuesta),
    )
)]
pub async fn qrs_por_fecha(
    State(banco): State<Banco>,
    TokenMarket(integrador): TokenMarket,
    CuerpoMarket(campos): CuerpoMarket,
) -> Result<Json<QrsDelDia>, ErrorMarket> {
    let dia = campos
        .fecha("generationDate")
        .map_err(ErrorMarket::cuerpo)?;
    let filas = banco.cobros_del_dia(integrador.cuenta_id, dia).await?;
    Ok(Json(QrsDelDia {
        detalles: filas.into_iter().map(|(f, a)| detalle(f, a)).collect(),
        success: true,
        message: None,
    }))
}

/// Estado de un QR. Úsalo para confirmar un pago antes de entregar lo vendido
/// (la notificación no va firmada en el BNB).
#[utoipa::path(
    post,
    path = "/QRSimple.API/api/v1/main/getQRStatusAsync",
    tag = "api-market",
    security(("tokenMarket" = [])),
    request_body = PedidoQrId,
    responses(
        (status = 200, description = "Estado del QR, o `success: false` si no existe", body = EstadoQr),
        (status = 400, description = "Cuerpo mal formado", body = Respuesta),
        (status = 401, description = "Token inválido o vencido", body = Respuesta),
    )
)]
pub async fn estado_qr(
    State(banco): State<Banco>,
    TokenMarket(integrador): TokenMarket,
    CuerpoMarket(campos): CuerpoMarket,
) -> Result<Json<EstadoQr>, ErrorMarket> {
    let numero = qr_id(&campos)?;
    let fila = banco
        .cobro_por_numero(integrador.cuenta_id, numero)
        .await?
        .ok_or_else(|| ErrorMarket::rechazo("QR no encontrado"))?;
    Ok(Json(EstadoQr {
        id: fila.number,
        status_id: market::estado_id(fila.estado()),
        expiration_date: market::fecha_local(fila.expires_at),
        voucher_id: fila.voucher_id,
        success: true,
        message: None,
    }))
}

/// Anula un QR que nadie pagó. Anular uno ya anulado no es error.
#[utoipa::path(
    post,
    path = "/QRSimple.API/api/v1/main/CancelQRByIdAsync",
    tag = "api-market",
    security(("tokenMarket" = [])),
    request_body = PedidoQrId,
    responses(
        (status = 200, description = "`success: true`, o `success: false` si ya se pagó, venció o no existe", body = Respuesta),
        (status = 400, description = "Cuerpo mal formado", body = Respuesta),
        (status = 401, description = "Token inválido o vencido", body = Respuesta),
    )
)]
pub async fn anular_qr(
    State(banco): State<Banco>,
    TokenMarket(integrador): TokenMarket,
    UrlBase(base): UrlBase,
    CuerpoMarket(campos): CuerpoMarket,
) -> Result<Json<Respuesta>, ErrorMarket> {
    let numero = qr_id(&campos)?;
    let fila = banco
        .cobro_por_numero(integrador.cuenta_id, numero)
        .await?
        .ok_or_else(|| ErrorMarket::rechazo("QR no encontrado"))?;
    match banco
        .anular_cobro(fila.id, integrador.cuenta_id, &base)
        .await
    {
        Ok(_) => Ok(Json(Respuesta {
            success: true,
            message: None,
        })),
        Err(ErrorBanco::QrYaPagado) => Err(ErrorMarket::rechazo(
            "El QR ya fue usado: no se puede anular",
        )),
        Err(ErrorBanco::QrVencido) => Err(ErrorMarket::rechazo("El QR ya expiró")),
        Err(otro) => Err(otro.into()),
    }
}
