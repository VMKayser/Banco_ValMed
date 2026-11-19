//! Lo que devuelve la API (y lo que viaja en los avisos). Los montos van como
//! string con 2 decimales y las fechas en ISO-8601 UTC.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Estado de un cobro QR. `EXPIRED` se calcula: es `ACTIVE` con `expiresAt` vencido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EstadoCobro {
    Active,
    Paid,
    Expired,
    Cancelled,
    Refunded,
}

impl EstadoCobro {
    pub fn como_texto(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Paid => "PAID",
            Self::Expired => "EXPIRED",
            Self::Cancelled => "CANCELLED",
            Self::Refunded => "REFUNDED",
        }
    }

    /// Ya no va a cambiar mientras alguien espera el pago.
    pub fn es_final(self) -> bool {
        !matches!(self, Self::Active)
    }

    pub fn desde_columna(status: &str, vence: DateTime<Utc>, ahora: DateTime<Utc>) -> Self {
        match status {
            "PAID" => Self::Paid,
            "CANCELLED" => Self::Cancelled,
            "REFUNDED" => Self::Refunded,
            _ if vence <= ahora => Self::Expired,
            _ => Self::Active,
        }
    }
}

/// Dueño de una cuenta, con el número enmascarado.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Titular {
    #[schema(example = "Comercio Demo")]
    pub name: String,
    #[schema(example = "****0005")]
    pub account: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pagador {
    /// C.I. completo del pagador (solo lo ve quien cobró).
    #[schema(example = "7000001")]
    pub ci: String,
    #[schema(example = "Rosa Quispe")]
    pub name: String,
    #[schema(example = "****0001")]
    pub account: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pago {
    /// Referencia única del pago en el banco; sirve para devolverlo.
    #[schema(example = "VM-20261118-000042")]
    pub bank_reference: String,
    /// Código de transacción de 10 caracteres (el `VoucherId` de la API Market).
    #[schema(example = "3JAO702813")]
    pub voucher_id: String,
    #[schema(value_type = String, example = "350.00")]
    pub amount: Decimal,
    #[schema(example = "BOB")]
    pub currency: String,
    pub paid_at: DateTime<Utc>,
    pub payer: Pagador,
    pub refunded_at: Option<DateTime<Utc>>,
    #[schema(example = "PAGADOR_DISTINTO")]
    pub refund_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Enlaces {
    /// PNG del QR (público, sirve para `<img src>`).
    pub image_png: String,
    /// SVG del QR (público).
    pub image_svg: String,
    /// Estado público del cobro (sin datos del pagador).
    pub status: String,
    /// Server-Sent Events: avisa en vivo cuando el cobro cambia de estado.
    pub events: String,
}

/// Cobro QR visto por quien cobra (dueño de la cuenta o su clave de API).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cobro {
    pub id: Uuid,
    /// Id numérico: el `id`/`qrId` de la API Market.
    #[schema(example = 51)]
    pub number: i64,
    /// Código para pagar sin cámara.
    #[schema(example = "7K2Q-M9XD")]
    pub code: String,
    pub status: EstadoCobro,
    #[schema(value_type = String, example = "350.00")]
    pub amount: Decimal,
    #[schema(example = "BOB")]
    pub currency: String,
    #[schema(example = "Entrada Guadalupe · Tramo A-014")]
    pub description: String,
    /// Referencia del integrador (única por cuenta); `null` en cobros hechos desde la app.
    #[schema(example = "orden-6f1c2a9e-intento-1")]
    pub merchant_reference: Option<String>,
    /// Lo que el integrador mandó al crear el cobro, devuelto tal cual.
    #[schema(value_type = Option<Object>)]
    pub metadata: Option<serde_json::Value>,
    pub receiver: Titular,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub refunded_at: Option<DateTime<Utc>>,
    /// Texto que va dentro del QR: una URL que abre la app del banco.
    #[schema(example = "http://localhost:8086/?p=0b8e4c3a-5d6f-4a1b-9c2d-7e8f9a0b1c2d")]
    pub payload: String,
    pub links: Enlaces,
    /// Solo en la respuesta de creación: PNG del QR en base64.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_png_base64: Option<String>,
    /// `null` hasta que alguien paga.
    pub payment: Option<Pago>,
}

/// Lo que cualquiera puede ver de un cobro conociendo su id (sin datos del pagador).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EstadoPublico {
    pub id: Uuid,
    #[schema(example = "7K2Q-M9XD")]
    pub code: String,
    pub status: EstadoCobro,
    #[schema(value_type = String, example = "350.00")]
    pub amount: Decimal,
    #[schema(example = "BOB")]
    pub currency: String,
    pub description: String,
    #[schema(example = "Comercio Demo")]
    pub receiver_name: String,
    pub expires_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
}

/// Comprobante que recibe quien paga.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Recibo {
    #[schema(example = "VM-20261118-000042")]
    pub bank_reference: String,
    #[schema(example = "3JAO702813")]
    pub voucher_id: String,
    pub qr_id: Uuid,
    #[schema(value_type = String, example = "350.00")]
    pub amount: Decimal,
    #[schema(example = "BOB")]
    pub currency: String,
    pub description: String,
    pub receiver: Titular,
    pub paid_at: DateTime<Utc>,
    /// Saldo de quien pagó después del pago.
    #[schema(value_type = String, example = "1650.00")]
    pub balance_after: Decimal,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Devolucion {
    pub id: Uuid,
    #[schema(example = "VM-20261118-000042")]
    pub bank_reference: String,
    #[schema(value_type = String, example = "350.00")]
    pub amount: Decimal,
    #[schema(example = "BOB")]
    pub currency: String,
    #[schema(example = "PAGADOR_DISTINTO")]
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cuenta {
    #[schema(example = "1001-000001")]
    pub number: String,
    #[schema(value_type = String, example = "2000.00")]
    pub balance: Decimal,
    #[schema(example = "BOB")]
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Perfil {
    pub customer_id: Uuid,
    #[schema(example = "Rosa Quispe")]
    pub full_name: String,
    #[schema(example = "7000001")]
    pub ci: String,
    pub account: Cuenta,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SesionCreada {
    /// Token opaco para `Authorization: Bearer`. Vence tras 15 min sin uso o 2 h en total.
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub profile: Perfil,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Movimiento {
    pub id: i64,
    /// `APERTURA`, `DEPOSITO`, `PAGO_QR`, `COBRO_QR` o `DEVOLUCION`.
    #[schema(example = "PAGO_QR")]
    pub kind: String,
    /// Con signo: negativo si salió dinero.
    #[schema(value_type = String, example = "-350.00")]
    pub amount: Decimal,
    #[schema(value_type = String, example = "1650.00")]
    pub balance_after: Decimal,
    pub description: String,
    #[schema(example = "Comercio Demo")]
    pub counterparty: Option<String>,
    #[schema(example = "VM-20261118-000042")]
    pub bank_reference: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaveApi {
    pub id: Uuid,
    #[schema(example = "SmartFila Ventas")]
    pub name: String,
    /// Primeros caracteres de la clave, para reconocerla.
    #[schema(example = "vm_test_Xy12ab")]
    pub prefix: String,
    /// `accountId` de la API Market (público).
    #[schema(example = "s9CG8FE7Id75ef2jeX9bUA==")]
    pub account_id: String,
    pub webhook_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Se muestra una sola vez: el banco no puede volver a mostrar la clave.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaveCreada {
    #[serde(flatten)]
    pub clave: ClaveApi,
    /// Va en la cabecera `X-Api-Key`.
    #[schema(example = "vm_test_Xy12ab…")]
    pub api_key: String,
    /// `authorizationId` inicial de la API Market: es la misma clave. Se puede
    /// cambiar con `UpdateCredentials` sin afectar a `X-Api-Key`.
    #[schema(example = "vm_test_Xy12ab…")]
    pub authorization_id: String,
    /// Secreto para verificar la firma de los avisos.
    #[schema(example = "whsec_…")]
    pub webhook_secret: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Aviso {
    pub id: Uuid,
    pub qr_id: Uuid,
    #[schema(example = "qr.paid")]
    pub event_type: String,
    /// `VALMED` (evento firmado) o `API_MARKET` (notificación con el formato del BNB).
    #[schema(example = "VALMED")]
    pub format: String,
    pub url: String,
    /// `PENDING`, `DELIVERED` o `FAILED`.
    #[schema(example = "DELIVERED")]
    pub status: String,
    pub attempts: i32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    /// Inicio de lo que respondió tu servidor (hasta 500 caracteres).
    #[schema(example = r#"{"success": true, "message": "OK"}"#)]
    pub last_response: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

/// Cuerpo de un aviso firmado.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventoAviso {
    /// Igual a la cabecera `X-ValMed-Event-Id`; se repite en los reintentos.
    pub id: Uuid,
    /// `qr.paid` o `qr.refunded`.
    #[serde(rename = "type")]
    #[schema(example = "qr.paid")]
    pub tipo: String,
    pub created_at: DateTime<Utc>,
    pub data: Cobro,
}

/// Notificación de pago con el formato del BNB (`POST ReceiveNotification`). Se
/// manda para los QR creados con la API Market. No trae monto ni C.I.: con
/// `QRId` buscas tu orden y confirmas con `getQRStatusAsync`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NotificacionMarket {
    /// El `id` que devolvió `getQRWithImageAsync`, como texto.
    #[serde(rename = "QRId")]
    #[schema(example = "76")]
    pub qr_id: String,
    /// El `gloss` con el que se creó el QR.
    #[serde(rename = "Gloss")]
    #[schema(example = "Entrada A-014")]
    pub gloss: String,
    /// Banco desde el que se pagó (en el simulador, siempre 1 = Banco ValMed).
    #[serde(rename = "sourceBankId")]
    #[schema(example = 1)]
    pub source_bank_id: i32,
    /// Titular de la cuenta que pagó, en mayúsculas.
    #[serde(rename = "originName")]
    #[schema(example = "ROSA QUISPE")]
    pub origin_name: String,
    /// Código de transacción de 10 caracteres.
    #[serde(rename = "VoucherId")]
    #[schema(example = "1J9F121212")]
    pub voucher_id: String,
    /// `dd/MM/yyyy HH:mm:ss`, hora de Bolivia.
    #[serde(rename = "TransactionDateTime")]
    #[schema(example = "18/11/2026 17:30:15")]
    pub transaction_date_time: String,
    /// El `additionalData` con el que se creó el QR (`""` si no se mandó).
    #[serde(rename = "additionalData")]
    #[schema(example = "orden-6f1c2a9e")]
    pub additional_data: String,
}
