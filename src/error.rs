//! Errores del banco y su cuerpo HTTP, el mismo de los servicios de SmartFila:
//! `{ code, message, details, traceId }`. Nunca se devuelven trazas de pila.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

tokio::task_local! {
    /// X-Correlation-Id de la petición en curso; lo pone el middleware de `api`.
    pub static CORRELACION: String;
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CuerpoError {
    #[schema(example = "INSUFFICIENT_FUNDS")]
    pub code: String,
    #[schema(example = "Saldo insuficiente")]
    pub message: String,
    pub details: Vec<Detalle>,
    /// Igual a la cabecera X-Correlation-Id, para buscar la petición en los logs.
    #[schema(example = "4bf92f35-77b3-4da6-a3ce-929d0e0e4736")]
    pub trace_id: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Detalle {
    #[schema(example = "amount")]
    pub field: String,
    #[schema(example = "máximo 2 decimales")]
    pub issue: String,
}

impl Detalle {
    pub fn new(field: &str, issue: impl Into<String>) -> Self {
        Self {
            field: field.to_owned(),
            issue: issue.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ErrorBanco {
    #[error("datos inválidos")]
    Validacion(Vec<Detalle>),
    #[error("{0}")]
    CuerpoInvalido(String),
    #[error("{0}")]
    NoAutenticado(&'static str),
    #[error("C.I. o PIN incorrectos")]
    CredencialesInvalidas,
    #[error("PIN incorrecto: te quedan {restantes} intentos")]
    PinIncorrecto { restantes: i32 },
    #[error("cuenta bloqueada por PIN incorrectos; intenta de nuevo en {segundos} s")]
    CuentaBloqueada { segundos: i64 },
    #[error("ese C.I. ya tiene una cuenta")]
    CiYaRegistrado,
    #[error("no existe ese cobro QR")]
    QrNoEncontrado,
    #[error("el cobro QR ya venció")]
    QrVencido,
    #[error("el cobro QR fue anulado")]
    QrAnulado,
    #[error("el cobro QR ya fue pagado")]
    QrYaPagado,
    #[error("el cobro QR no está pagado")]
    QrNoPagado,
    #[error("no puedes pagar un cobro de tu propia cuenta")]
    QrPropio,
    #[error("saldo insuficiente")]
    SaldoInsuficiente,
    #[error("ya existe un cobro con esa merchantReference y otros datos")]
    ReferenciaEnConflicto,
    #[error("no existe ese pago")]
    PagoNoEncontrado,
    #[error("no existe esa clave de API")]
    ClaveNoEncontrada,
    #[error("llegaste al máximo de {0} claves activas")]
    LimiteDeClaves(i64),
    #[error("no existe ese aviso")]
    AvisoNoEncontrado,
    #[error("recurso no encontrado")]
    NoEncontrado,
    #[error("base de datos: {0}")]
    BaseDatos(#[from] sqlx::Error),
    #[error("error interno: {0:#}")]
    Interno(#[from] anyhow::Error),
}

impl ErrorBanco {
    pub fn validacion(campo: &str, problema: impl Into<String>) -> Self {
        Self::Validacion(vec![Detalle::new(campo, problema)])
    }

    /// Estado HTTP y código estable que leen los clientes.
    pub fn estado_y_codigo(&self) -> (StatusCode, &'static str) {
        use ErrorBanco::*;
        match self {
            Validacion(_) | CuerpoInvalido(_) => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
            NoAutenticado(_) => (StatusCode::UNAUTHORIZED, "UNAUTHENTICATED"),
            CredencialesInvalidas => (StatusCode::UNAUTHORIZED, "INVALID_CREDENTIALS"),
            PinIncorrecto { .. } => (StatusCode::UNAUTHORIZED, "INVALID_PIN"),
            CuentaBloqueada { .. } => (StatusCode::LOCKED, "ACCOUNT_LOCKED"),
            CiYaRegistrado => (StatusCode::CONFLICT, "CI_ALREADY_REGISTERED"),
            QrNoEncontrado => (StatusCode::NOT_FOUND, "QR_NOT_FOUND"),
            QrVencido => (StatusCode::CONFLICT, "QR_EXPIRED"),
            QrAnulado => (StatusCode::CONFLICT, "QR_CANCELLED"),
            QrYaPagado => (StatusCode::CONFLICT, "QR_ALREADY_PAID"),
            QrNoPagado => (StatusCode::CONFLICT, "QR_NOT_PAID"),
            QrPropio => (StatusCode::CONFLICT, "CANNOT_PAY_OWN_QR"),
            SaldoInsuficiente => (StatusCode::CONFLICT, "INSUFFICIENT_FUNDS"),
            ReferenciaEnConflicto => (StatusCode::CONFLICT, "MERCHANT_REFERENCE_CONFLICT"),
            PagoNoEncontrado => (StatusCode::NOT_FOUND, "PAYMENT_NOT_FOUND"),
            ClaveNoEncontrada => (StatusCode::NOT_FOUND, "API_KEY_NOT_FOUND"),
            LimiteDeClaves(_) => (StatusCode::CONFLICT, "API_KEY_LIMIT"),
            AvisoNoEncontrado => (StatusCode::NOT_FOUND, "WEBHOOK_DELIVERY_NOT_FOUND"),
            NoEncontrado => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            BaseDatos(e) if base_caida(e) => {
                (StatusCode::SERVICE_UNAVAILABLE, "DEPENDENCY_UNAVAILABLE")
            }
            BaseDatos(_) | Interno(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        }
    }

    /// ¿Violó esta restricción `UNIQUE` (o índice único) de PostgreSQL?
    pub fn es_unico(e: &sqlx::Error, restriccion: &str) -> bool {
        matches!(e, sqlx::Error::Database(d)
            if d.is_unique_violation() && d.constraint() == Some(restriccion))
    }
}

fn base_caida(e: &sqlx::Error) -> bool {
    matches!(
        e,
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::PoolClosed
    )
}

impl From<JsonRejection> for ErrorBanco {
    fn from(r: JsonRejection) -> Self {
        Self::CuerpoInvalido(r.body_text())
    }
}

impl From<QueryRejection> for ErrorBanco {
    fn from(r: QueryRejection) -> Self {
        Self::CuerpoInvalido(r.body_text())
    }
}

impl From<PathRejection> for ErrorBanco {
    /// Un id mal formado se responde igual que uno que no existe.
    fn from(_: PathRejection) -> Self {
        Self::NoEncontrado
    }
}

impl IntoResponse for ErrorBanco {
    fn into_response(self) -> Response {
        let (estado, code) = self.estado_y_codigo();
        let (message, details) = match &self {
            Self::Validacion(detalles) => ("Datos inválidos".to_owned(), detalles.clone()),
            Self::BaseDatos(e) if base_caida(e) => {
                tracing::error!(error = %e, "base de datos no disponible");
                (
                    "Servicio temporalmente no disponible".to_owned(),
                    Vec::new(),
                )
            }
            Self::BaseDatos(_) | Self::Interno(_) => {
                tracing::error!(error = %self, "error interno");
                ("Error interno".to_owned(), Vec::new())
            }
            otro => (capitalizar(&otro.to_string()), Vec::new()),
        };

        let cuerpo = CuerpoError {
            code: code.to_owned(),
            message,
            details,
            trace_id: CORRELACION.try_with(Clone::clone).unwrap_or_default(),
        };
        let mut respuesta = (estado, Json(cuerpo)).into_response();
        if estado == StatusCode::UNAUTHORIZED && matches!(self, Self::NoAutenticado(_)) {
            respuesta
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        respuesta
    }
}

pub(crate) fn capitalizar(texto: &str) -> String {
    let mut letras = texto.chars();
    match letras.next() {
        Some(primera) => primera.to_uppercase().chain(letras).collect(),
        None => String::new(),
    }
}
