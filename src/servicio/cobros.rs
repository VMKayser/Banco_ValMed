//! Cobros QR dinámicos: crear, consultar, pagar, anular y devolver.
//!
//! Pagar es una sola transacción: se bloquea el cobro (`FOR UPDATE`), se bloquean
//! las dos cuentas en orden, se debita con `balance >= monto` (si no alcanza no
//! cambia nada), se acredita, se registra el pago, los dos movimientos y, si el
//! cobro vino de una clave de API con webhook, la fila del aviso (outbox).

use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use uuid::Uuid;

use super::vistas::{
    Cobro, Devolucion, Enlaces, EstadoCobro, EstadoPublico, EventoAviso, NotificacionMarket,
    Pagador, Pago, Recibo, Titular,
};
use super::{bloquear_cuentas, enmascarar_cuenta, Banco};
use crate::dominio::{ci, codigo, dinero, market};
use crate::error::ErrorBanco;

pub const VIGENCIA_POR_DEFECTO_S: i64 = 600;
pub const VIGENCIA_MINIMA_S: i64 = 30;
pub const VIGENCIA_MAXIMA_S: i64 = 86_400;

/// Motivos de devolución aceptados.
pub const MOTIVOS_DEVOLUCION: [&str; 4] = [
    "PAGADOR_DISTINTO",
    "PAGO_TARDIO",
    "MONTO_DISTINTO",
    "DEVOLUCION",
];

/// (pago, cobro, cuenta del pagador, monto, moneda, devuelto en)
type FilaPago = (Uuid, Uuid, Uuid, Decimal, String, Option<DateTime<Utc>>);

/// Datos ya validados para crear un cobro.
pub struct DatosCobro {
    pub referencia: Option<String>,
    pub monto: Decimal,
    pub descripcion: String,
    pub vence_en: DateTime<Utc>,
    pub metadata: Option<Value>,
    /// Creado con la API Market: avisa con el formato del BNB.
    pub api_market: bool,
    pub datos_adicionales: Option<String>,
}

fn validar_descripcion(campo: &str, texto: &str) -> Result<String, ErrorBanco> {
    let texto = texto.trim().to_owned();
    if !(1..=140).contains(&texto.chars().count()) {
        return Err(ErrorBanco::validacion(campo, "entre 1 y 140 caracteres"));
    }
    Ok(texto)
}

impl DatosCobro {
    /// Valida lo que llega por la API o la app.
    pub fn validar(
        referencia: Option<String>,
        monto: Decimal,
        moneda: Option<&str>,
        descripcion: &str,
        vigencia_s: Option<i64>,
        vence_en: Option<DateTime<Utc>>,
        metadata: Option<Value>,
    ) -> Result<Self, ErrorBanco> {
        let monto = dinero::monto("amount", monto, dinero::maximo_cobro())?;
        dinero::moneda(moneda)?;

        let descripcion = validar_descripcion("description", descripcion)?;

        let referencia = referencia.map(|r| r.trim().to_owned());
        if let Some(r) = &referencia {
            if !(1..=100).contains(&r.chars().count()) || r.chars().any(char::is_control) {
                return Err(ErrorBanco::validacion(
                    "merchantReference",
                    "entre 1 y 100 caracteres, sin saltos de línea",
                ));
            }
        }

        let ahora = Utc::now();
        let vence_en = match (vigencia_s, vence_en) {
            (Some(_), Some(_)) => {
                return Err(ErrorBanco::validacion(
                    "expiresAt",
                    "usa expiresAt o expiresInSeconds, no los dos",
                ))
            }
            (Some(s), None) => {
                if !(VIGENCIA_MINIMA_S..=VIGENCIA_MAXIMA_S).contains(&s) {
                    return Err(ErrorBanco::validacion(
                        "expiresInSeconds",
                        format!("entre {VIGENCIA_MINIMA_S} y {VIGENCIA_MAXIMA_S}"),
                    ));
                }
                ahora + Duration::seconds(s)
            }
            (None, Some(fecha)) => {
                let segundos = (fecha - ahora).num_seconds();
                if !(VIGENCIA_MINIMA_S..=VIGENCIA_MAXIMA_S).contains(&segundos) {
                    return Err(ErrorBanco::validacion(
                        "expiresAt",
                        "entre 30 s y 24 h desde ahora",
                    ));
                }
                fecha
            }
            (None, None) => ahora + Duration::seconds(VIGENCIA_POR_DEFECTO_S),
        };

        if let Some(m) = &metadata {
            if !m.is_object() {
                return Err(ErrorBanco::validacion(
                    "metadata",
                    "debe ser un objeto JSON",
                ));
            }
            if m.to_string().len() > 2_000 {
                return Err(ErrorBanco::validacion("metadata", "máximo 2000 bytes"));
            }
        }

        Ok(Self {
            referencia,
            monto,
            descripcion,
            vence_en,
            metadata,
            api_market: false,
            datos_adicionales: None,
        })
    }

    /// Cobro de la API Market: `gloss`, `amount`, vencimiento ya calculado desde
    /// `expirationDate` y `additionalData` opcional.
    pub fn api_market(
        monto: Decimal,
        glosa: &str,
        vence_en: DateTime<Utc>,
        datos_adicionales: Option<String>,
    ) -> Result<Self, ErrorBanco> {
        let monto = dinero::monto("amount", monto, dinero::maximo_cobro())?;
        let descripcion = validar_descripcion("gloss", glosa)?;
        let datos_adicionales = datos_adicionales.filter(|d| !d.is_empty());
        if datos_adicionales
            .as_ref()
            .is_some_and(|d| d.chars().count() > 500)
        {
            return Err(ErrorBanco::validacion(
                "additionalData",
                "máximo 500 caracteres",
            ));
        }
        Ok(Self {
            referencia: None,
            monto,
            descripcion,
            vence_en,
            metadata: None,
            api_market: true,
            datos_adicionales,
        })
    }
}

/// Fila de un cobro con su dueño, su pago y el pagador (si los hay).
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct FilaCobro {
    pub id: Uuid,
    pub number: i64,
    pub channel: String,
    pub additional_data: Option<String>,
    pub code: String,
    pub status: String,
    pub amount: Decimal,
    pub currency: String,
    pub description: String,
    pub merchant_reference: Option<String>,
    pub metadata: Option<Value>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub refunded_at: Option<DateTime<Utc>>,
    pub account_id: Uuid,
    pub receiver_name: String,
    pub receiver_account: String,
    pub bank_reference: Option<String>,
    pub voucher_id: Option<String>,
    pub payer_account_id: Option<Uuid>,
    pub payer_ci_number: Option<String>,
    pub payer_ci_complement: Option<String>,
    pub payer_name: Option<String>,
    pub payer_account: Option<String>,
    pub refund_reason: Option<String>,
}

pub(crate) const SELECT_COBRO: &str = "
    SELECT q.id, q.number, q.channel, q.additional_data, q.code, q.status, q.amount, q.currency, q.description, q.merchant_reference,
           q.metadata, q.expires_at, q.created_at, q.paid_at, q.cancelled_at, q.refunded_at,
           q.account_id,
           rc.full_name AS receiver_name, ra.number AS receiver_account,
           p.bank_reference, p.voucher_id, p.payer_account_id,
           pc.ci_number AS payer_ci_number, pc.ci_complement AS payer_ci_complement,
           pc.full_name AS payer_name, pa.number AS payer_account,
           r.reason AS refund_reason
    FROM qr_charges q
    JOIN accounts ra ON ra.id = q.account_id
    JOIN customers rc ON rc.id = ra.customer_id
    LEFT JOIN payments p ON p.qr_charge_id = q.id
    LEFT JOIN accounts pa ON pa.id = p.payer_account_id
    LEFT JOIN customers pc ON pc.id = pa.customer_id
    LEFT JOIN refunds r ON r.payment_id = p.id";

impl FilaCobro {
    pub fn estado(&self) -> EstadoCobro {
        EstadoCobro::desde_columna(&self.status, self.expires_at, Utc::now())
    }

    pub fn vista(self, base: &str) -> Cobro {
        let estado = self.estado();
        let pago = match (&self.bank_reference, self.paid_at) {
            (Some(referencia), Some(pagado)) => Some(Pago {
                bank_reference: referencia.clone(),
                voucher_id: self.voucher_id.clone().unwrap_or_default(),
                amount: self.amount,
                currency: self.currency.clone(),
                paid_at: pagado,
                payer: Pagador {
                    ci: ci::formatear(
                        self.payer_ci_number.as_deref().unwrap_or_default(),
                        self.payer_ci_complement.as_deref(),
                    ),
                    name: self.payer_name.clone().unwrap_or_default(),
                    account: enmascarar_cuenta(self.payer_account.as_deref().unwrap_or_default()),
                },
                refunded_at: self.refunded_at,
                refund_reason: self.refund_reason.clone(),
            }),
            _ => None,
        };
        Cobro {
            id: self.id,
            number: self.number,
            code: codigo::formatear(&self.code),
            status: estado,
            amount: self.amount,
            currency: self.currency,
            description: self.description,
            merchant_reference: self.merchant_reference,
            metadata: self.metadata,
            receiver: Titular {
                name: self.receiver_name,
                account: enmascarar_cuenta(&self.receiver_account),
            },
            created_at: self.created_at,
            expires_at: self.expires_at,
            paid_at: self.paid_at,
            cancelled_at: self.cancelled_at,
            refunded_at: self.refunded_at,
            payload: payload(base, self.id),
            links: enlaces(base, self.id),
            image_png_base64: None,
            payment: pago,
        }
    }

    /// Cuerpo de `ReceiveNotification` (solo si ya se pagó).
    pub fn notificacion_market(&self) -> Option<NotificacionMarket> {
        Some(NotificacionMarket {
            qr_id: self.number.to_string(),
            gloss: self.description.clone(),
            source_bank_id: market::ID_BANCO_VALMED,
            origin_name: self.payer_name.as_deref()?.to_uppercase(),
            voucher_id: self.voucher_id.clone()?,
            transaction_date_time: market::fecha_notificacion(self.paid_at?),
            additional_data: self.additional_data.clone().unwrap_or_default(),
        })
    }

    pub fn publico(&self) -> EstadoPublico {
        EstadoPublico {
            id: self.id,
            code: codigo::formatear(&self.code),
            status: self.estado(),
            amount: self.amount,
            currency: self.currency.clone(),
            description: self.description.clone(),
            receiver_name: self.receiver_name.clone(),
            expires_at: self.expires_at,
            paid_at: self.paid_at,
        }
    }
}

/// Lo que va dentro del QR.
pub fn payload(base: &str, id: Uuid) -> String {
    format!("{base}/?p={id}")
}

pub fn enlaces(base: &str, id: Uuid) -> Enlaces {
    let raiz = format!("{base}/api/v1/public/qr/{id}");
    Enlaces {
        image_png: format!("{raiz}/image.png"),
        image_svg: format!("{raiz}/image.svg"),
        events: format!("{raiz}/events"),
        status: raiz,
    }
}

async fn cargar(
    ejecutor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<FilaCobro>, sqlx::Error> {
    sqlx::query_as::<_, FilaCobro>(&format!("{SELECT_COBRO} WHERE q.id = $1"))
        .bind(id)
        .fetch_optional(ejecutor)
        .await
}

impl Banco {
    /// Crea un cobro para `cuenta_id`. Con `merchantReference` repetida y los
    /// mismos datos devuelve el existente (`false` = no se creó uno nuevo).
    pub async fn crear_cobro(
        &self,
        cuenta_id: Uuid,
        clave_id: Option<Uuid>,
        datos: DatosCobro,
        base: &str,
    ) -> Result<(Cobro, bool), ErrorBanco> {
        if let Some(referencia) = &datos.referencia {
            if let Some(existente) = self.cobro_por_referencia(cuenta_id, referencia).await? {
                return mismo_cobro(existente, &datos, base).map(|c| (c, false));
            }
        }

        for _ in 0..5 {
            let id = Uuid::new_v4();
            let canal = match (datos.api_market, clave_id) {
                (true, _) => "API_MARKET",
                (false, Some(_)) => "API",
                (false, None) => "APP",
            };
            let resultado = sqlx::query(
                "INSERT INTO qr_charges (id, code, account_id, api_key_id, merchant_reference,
                                         amount, description, metadata, expires_at, channel,
                                         additional_data)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(id)
            .bind(codigo::generar())
            .bind(cuenta_id)
            .bind(clave_id)
            .bind(&datos.referencia)
            .bind(datos.monto)
            .bind(&datos.descripcion)
            .bind(&datos.metadata)
            .bind(datos.vence_en)
            .bind(canal)
            .bind(&datos.datos_adicionales)
            .execute(self.pool())
            .await;

            match resultado {
                Ok(_) => {
                    let fila = cargar(self.pool(), id)
                        .await?
                        .ok_or(ErrorBanco::QrNoEncontrado)?;
                    return Ok((fila.vista(base), true));
                }
                Err(e) if ErrorBanco::es_unico(&e, "uk_qr_code") => continue,
                Err(e) if ErrorBanco::es_unico(&e, "uk_qr_reference") => {
                    // Otra petición con la misma referencia ganó la carrera.
                    let referencia = datos.referencia.as_deref().unwrap_or_default();
                    let existente = self
                        .cobro_por_referencia(cuenta_id, referencia)
                        .await?
                        .ok_or(ErrorBanco::ReferenciaEnConflicto)?;
                    return mismo_cobro(existente, &datos, base).map(|c| (c, false));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(anyhow!("no se pudo generar un código de cobro único").into())
    }

    async fn cobro_por_referencia(
        &self,
        cuenta_id: Uuid,
        referencia: &str,
    ) -> Result<Option<FilaCobro>, ErrorBanco> {
        Ok(sqlx::query_as::<_, FilaCobro>(&format!(
            "{SELECT_COBRO} WHERE q.account_id = $1 AND q.merchant_reference = $2"
        ))
        .bind(cuenta_id)
        .bind(referencia)
        .fetch_optional(self.pool())
        .await?)
    }

    /// Cualquier cobro por id (para vistas públicas).
    pub(crate) async fn fila_cobro(&self, id: Uuid) -> Result<FilaCobro, ErrorBanco> {
        cargar(self.pool(), id)
            .await?
            .ok_or(ErrorBanco::QrNoEncontrado)
    }

    /// Un cobro de esta cuenta; si es de otra, responde como si no existiera.
    pub async fn cobro_de_cuenta(
        &self,
        id: Uuid,
        cuenta_id: Uuid,
        base: &str,
    ) -> Result<Cobro, ErrorBanco> {
        let fila = self.fila_cobro(id).await?;
        if fila.account_id != cuenta_id {
            return Err(ErrorBanco::QrNoEncontrado);
        }
        Ok(fila.vista(base))
    }

    pub async fn estado_publico(&self, id: Uuid) -> Result<EstadoPublico, ErrorBanco> {
        Ok(self.fila_cobro(id).await?.publico())
    }

    /// Por código corto: solo cobros activos (el código tiene pocos bits).
    pub async fn cobro_activo_por_codigo(&self, texto: &str) -> Result<EstadoPublico, ErrorBanco> {
        let codigo = codigo::normalizar(texto).ok_or(ErrorBanco::QrNoEncontrado)?;
        let fila = sqlx::query_as::<_, FilaCobro>(&format!(
            "{SELECT_COBRO} WHERE q.code = $1 AND q.status = 'ACTIVE' AND q.expires_at > now()"
        ))
        .bind(codigo)
        .fetch_optional(self.pool())
        .await?
        .ok_or(ErrorBanco::QrNoEncontrado)?;
        Ok(fila.publico())
    }

    pub async fn listar_cobros(
        &self,
        cuenta_id: Uuid,
        referencia: Option<&str>,
        limite: i64,
        base: &str,
    ) -> Result<Vec<Cobro>, ErrorBanco> {
        let filas = sqlx::query_as::<_, FilaCobro>(&format!(
            "{SELECT_COBRO}
             WHERE q.account_id = $1 AND ($2::text IS NULL OR q.merchant_reference = $2)
             ORDER BY q.created_at DESC
             LIMIT $3"
        ))
        .bind(cuenta_id)
        .bind(referencia)
        .bind(limite.clamp(1, 100))
        .fetch_all(self.pool())
        .await?;
        Ok(filas.into_iter().map(|f| f.vista(base)).collect())
    }

    /// Anula un cobro activo. Anular uno ya anulado no es error.
    pub async fn anular_cobro(
        &self,
        id: Uuid,
        cuenta_id: Uuid,
        base: &str,
    ) -> Result<Cobro, ErrorBanco> {
        let anulado = sqlx::query(
            "UPDATE qr_charges SET status = 'CANCELLED', cancelled_at = now()
             WHERE id = $1 AND account_id = $2 AND status = 'ACTIVE' AND expires_at > now()",
        )
        .bind(id)
        .bind(cuenta_id)
        .execute(self.pool())
        .await?
        .rows_affected()
            == 1;

        let fila = self.fila_cobro(id).await?;
        if fila.account_id != cuenta_id {
            return Err(ErrorBanco::QrNoEncontrado);
        }
        match fila.estado() {
            EstadoCobro::Cancelled => {
                if anulado {
                    self.publicar(id, EstadoCobro::Cancelled);
                }
                Ok(fila.vista(base))
            }
            EstadoCobro::Paid | EstadoCobro::Refunded => Err(ErrorBanco::QrYaPagado),
            EstadoCobro::Expired => Err(ErrorBanco::QrVencido),
            EstadoCobro::Active => Err(anyhow!("el cobro {id} sigue activo tras anularlo").into()),
        }
    }

    /// Paga un cobro desde la cuenta de `cliente_id`. Con `pin = None` no se pide
    /// PIN (sandbox). Si esta misma cuenta ya lo pagó, devuelve el mismo recibo.
    pub async fn pagar(
        &self,
        id: Uuid,
        cliente_id: Uuid,
        pin: Option<&str>,
        base: &str,
    ) -> Result<(Recibo, bool), ErrorBanco> {
        if let Some(pin) = pin {
            self.verificar_pin(cliente_id, pin, false).await?;
        }

        let mut tx = self.pool().begin().await?;
        let (cuenta_pagador, nombre_pagador): (Uuid, String) = sqlx::query_as(
            "SELECT a.id, c.full_name FROM accounts a JOIN customers c ON c.id = a.customer_id
             WHERE c.id = $1",
        )
        .bind(cliente_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ErrorBanco::NoAutenticado("la sesión ya no es válida"))?;

        let bloqueado: Option<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT account_id, status, expires_at FROM qr_charges WHERE id = $1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let (cuenta_receptor, status, vence) = bloqueado.ok_or(ErrorBanco::QrNoEncontrado)?;

        match EstadoCobro::desde_columna(&status, vence, Utc::now()) {
            EstadoCobro::Active => {}
            EstadoCobro::Paid | EstadoCobro::Refunded => {
                drop(tx);
                return self.recibo_si_es_suyo(id, cuenta_pagador).await;
            }
            EstadoCobro::Cancelled => return Err(ErrorBanco::QrAnulado),
            EstadoCobro::Expired => return Err(ErrorBanco::QrVencido),
        }
        if cuenta_receptor == cuenta_pagador {
            return Err(ErrorBanco::QrPropio);
        }

        bloquear_cuentas(&mut tx, &[cuenta_pagador, cuenta_receptor]).await?;

        let fila = cargar(&mut *tx, id)
            .await?
            .ok_or(ErrorBanco::QrNoEncontrado)?;
        let monto = fila.amount;

        let saldo_pagador: Option<Decimal> = sqlx::query_scalar(
            "UPDATE accounts SET balance = balance - $2 WHERE id = $1 AND balance >= $2
             RETURNING balance",
        )
        .bind(cuenta_pagador)
        .bind(monto)
        .fetch_optional(&mut *tx)
        .await?;
        let saldo_pagador = saldo_pagador.ok_or(ErrorBanco::SaldoInsuficiente)?;

        let saldo_receptor: Decimal = sqlx::query_scalar(
            "UPDATE accounts SET balance = balance + $2 WHERE id = $1 RETURNING balance",
        )
        .bind(cuenta_receptor)
        .bind(monto)
        .fetch_one(&mut *tx)
        .await?;

        let pago_id = Uuid::new_v4();
        let voucher = codigo::voucher();
        let (referencia, pagado): (String, DateTime<Utc>) = sqlx::query_as(
            "INSERT INTO payments (id, bank_reference, qr_charge_id, payer_account_id, amount,
                                   voucher_id)
             VALUES ($1,
                     'VM-' || to_char(now() AT TIME ZONE 'America/La_Paz', 'YYYYMMDD') || '-'
                           || lpad(nextval('bank_reference_seq')::text, 6, '0'),
                     $2, $3, $4, $5)
             RETURNING bank_reference, paid_at",
        )
        .bind(pago_id)
        .bind(id)
        .bind(cuenta_pagador)
        .bind(monto)
        .bind(&voucher)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query("UPDATE qr_charges SET status = 'PAID', paid_at = $2 WHERE id = $1")
            .bind(id)
            .bind(pagado)
            .execute(&mut *tx)
            .await?;

        movimiento(
            &mut tx,
            cuenta_pagador,
            "PAGO_QR",
            -monto,
            saldo_pagador,
            &fila.description,
            &fila.receiver_name,
            pago_id,
            None,
        )
        .await?;
        movimiento(
            &mut tx,
            cuenta_receptor,
            "COBRO_QR",
            monto,
            saldo_receptor,
            &fila.description,
            &nombre_pagador,
            pago_id,
            None,
        )
        .await?;

        self.encolar_aviso(&mut tx, id, "qr.paid", base).await?;
        tx.commit().await?;

        self.publicar(id, EstadoCobro::Paid);
        tracing::info!(qr = %id, referencia = %referencia, "cobro QR pagado");

        Ok((
            Recibo {
                bank_reference: referencia,
                voucher_id: voucher,
                qr_id: id,
                amount: monto,
                currency: fila.currency.clone(),
                description: fila.description.clone(),
                receiver: Titular {
                    name: fila.receiver_name.clone(),
                    account: enmascarar_cuenta(&fila.receiver_account),
                },
                paid_at: pagado,
                balance_after: saldo_pagador,
            },
            true,
        ))
    }

    /// Reintento del mismo pagador (doble toque o red caída): mismo recibo.
    async fn recibo_si_es_suyo(
        &self,
        id: Uuid,
        cuenta_pagador: Uuid,
    ) -> Result<(Recibo, bool), ErrorBanco> {
        let fila = self.fila_cobro(id).await?;
        if fila.payer_account_id != Some(cuenta_pagador) || fila.estado() != EstadoCobro::Paid {
            return Err(ErrorBanco::QrYaPagado);
        }
        let saldo: Decimal = sqlx::query_scalar("SELECT balance FROM accounts WHERE id = $1")
            .bind(cuenta_pagador)
            .fetch_one(self.pool())
            .await?;
        Ok((
            Recibo {
                bank_reference: fila.bank_reference.clone().unwrap_or_default(),
                voucher_id: fila.voucher_id.clone().unwrap_or_default(),
                qr_id: id,
                amount: fila.amount,
                currency: fila.currency.clone(),
                description: fila.description.clone(),
                receiver: Titular {
                    name: fila.receiver_name.clone(),
                    account: enmascarar_cuenta(&fila.receiver_account),
                },
                paid_at: fila.paid_at.unwrap_or_else(Utc::now),
                balance_after: saldo,
            },
            false,
        ))
    }

    /// Devuelve el pago completo a quien pagó. Si ya se devolvió, responde la misma
    /// devolución (`false`).
    pub async fn devolver(
        &self,
        referencia: &str,
        cuenta_id: Uuid,
        motivo: &str,
        base: &str,
    ) -> Result<(Devolucion, bool), ErrorBanco> {
        let mut tx = self.pool().begin().await?;
        let pago: Option<FilaPago> = sqlx::query_as(
            "SELECT p.id, p.qr_charge_id, p.payer_account_id, p.amount, q.currency, p.refunded_at
                 FROM payments p JOIN qr_charges q ON q.id = p.qr_charge_id
                 WHERE p.bank_reference = $1 AND q.account_id = $2
                 FOR UPDATE OF p, q",
        )
        .bind(referencia)
        .bind(cuenta_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (pago_id, qr_id, cuenta_pagador, monto, moneda, devuelto) =
            pago.ok_or(ErrorBanco::PagoNoEncontrado)?;

        if devuelto.is_some() {
            let (id, motivo, creada): (Uuid, String, DateTime<Utc>) =
                sqlx::query_as("SELECT id, reason, created_at FROM refunds WHERE payment_id = $1")
                    .bind(pago_id)
                    .fetch_one(&mut *tx)
                    .await?;
            return Ok((
                Devolucion {
                    id,
                    bank_reference: referencia.to_owned(),
                    amount: monto,
                    currency: moneda,
                    reason: motivo,
                    created_at: creada,
                },
                false,
            ));
        }

        bloquear_cuentas(&mut tx, &[cuenta_id, cuenta_pagador]).await?;
        let fila = cargar(&mut *tx, qr_id)
            .await?
            .ok_or(ErrorBanco::QrNoEncontrado)?;

        let saldo_receptor: Option<Decimal> = sqlx::query_scalar(
            "UPDATE accounts SET balance = balance - $2 WHERE id = $1 AND balance >= $2
             RETURNING balance",
        )
        .bind(cuenta_id)
        .bind(monto)
        .fetch_optional(&mut *tx)
        .await?;
        let saldo_receptor = saldo_receptor.ok_or(ErrorBanco::SaldoInsuficiente)?;
        let saldo_pagador: Decimal = sqlx::query_scalar(
            "UPDATE accounts SET balance = balance + $2 WHERE id = $1 RETURNING balance",
        )
        .bind(cuenta_pagador)
        .bind(monto)
        .fetch_one(&mut *tx)
        .await?;

        let devolucion_id = Uuid::new_v4();
        let creada: DateTime<Utc> = sqlx::query_scalar(
            "INSERT INTO refunds (id, payment_id, reason, amount) VALUES ($1, $2, $3, $4)
             RETURNING created_at",
        )
        .bind(devolucion_id)
        .bind(pago_id)
        .bind(motivo)
        .bind(monto)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("UPDATE payments SET refunded_at = $2 WHERE id = $1")
            .bind(pago_id)
            .bind(creada)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE qr_charges SET status = 'REFUNDED', refunded_at = $2 WHERE id = $1")
            .bind(qr_id)
            .bind(creada)
            .execute(&mut *tx)
            .await?;

        let descripcion = format!("Devolución: {}", fila.description);
        movimiento(
            &mut tx,
            cuenta_id,
            "DEVOLUCION",
            -monto,
            saldo_receptor,
            &descripcion,
            fila.payer_name.as_deref().unwrap_or_default(),
            pago_id,
            Some(devolucion_id),
        )
        .await?;
        movimiento(
            &mut tx,
            cuenta_pagador,
            "DEVOLUCION",
            monto,
            saldo_pagador,
            &descripcion,
            &fila.receiver_name,
            pago_id,
            Some(devolucion_id),
        )
        .await?;

        self.encolar_aviso(&mut tx, qr_id, "qr.refunded", base)
            .await?;
        tx.commit().await?;
        self.publicar(qr_id, EstadoCobro::Refunded);
        tracing::info!(qr = %qr_id, referencia, motivo, "pago devuelto");

        Ok((
            Devolucion {
                id: devolucion_id,
                bank_reference: referencia.to_owned(),
                amount: monto,
                currency: moneda,
                reason: motivo.to_owned(),
                created_at: creada,
            },
            true,
        ))
    }

    /// Si el cobro salió de una clave de API con webhook, guarda el aviso en la
    /// misma transacción (outbox). El trabajador de `avisos` lo envía después.
    /// Los cobros de la API Market avisan solo el pago, con el formato del BNB.
    async fn encolar_aviso(
        &self,
        tx: &mut sqlx::PgConnection,
        qr_id: Uuid,
        tipo: &str,
        base: &str,
    ) -> Result<(), ErrorBanco> {
        let destino: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT k.id, k.webhook_url FROM qr_charges q JOIN api_keys k ON k.id = q.api_key_id
             WHERE q.id = $1 AND k.revoked_at IS NULL AND k.webhook_url IS NOT NULL",
        )
        .bind(qr_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((clave_id, url)) = destino else {
            return Ok(());
        };

        let fila = cargar(&mut *tx, qr_id)
            .await?
            .ok_or(ErrorBanco::QrNoEncontrado)?;
        let aviso_id = Uuid::new_v4();
        let (formato, cuerpo) = if fila.channel == "API_MARKET" {
            if tipo != "qr.paid" {
                return Ok(());
            }
            let notificacion = fila
                .notificacion_market()
                .ok_or_else(|| anyhow!("el cobro {qr_id} no tiene pago para notificar"))?;
            ("API_MARKET", serde_json::to_string(&notificacion))
        } else {
            (
                "VALMED",
                serde_json::to_string(&EventoAviso {
                    id: aviso_id,
                    tipo: tipo.to_owned(),
                    created_at: Utc::now(),
                    data: fila.vista(base),
                }),
            )
        };
        let cuerpo = cuerpo.map_err(|e| anyhow!(e))?;

        sqlx::query(
            "INSERT INTO webhook_deliveries (id, api_key_id, qr_charge_id, event_type, url, body,
                                             format)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(aviso_id)
        .bind(clave_id)
        .bind(qr_id)
        .bind(tipo)
        .bind(url)
        .bind(cuerpo)
        .bind(formato)
        .execute(&mut *tx)
        .await?;
        Ok(())
    }
}

fn mismo_cobro(existente: FilaCobro, datos: &DatosCobro, base: &str) -> Result<Cobro, ErrorBanco> {
    if existente.amount == datos.monto && existente.description == datos.descripcion {
        Ok(existente.vista(base))
    } else {
        Err(ErrorBanco::ReferenciaEnConflicto)
    }
}

#[allow(clippy::too_many_arguments)]
async fn movimiento(
    tx: &mut sqlx::PgConnection,
    cuenta: Uuid,
    tipo: &str,
    monto: Decimal,
    saldo: Decimal,
    descripcion: &str,
    contraparte: &str,
    pago: Uuid,
    devolucion: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ledger_entries
             (account_id, kind, amount, balance_after, description, counterparty, payment_id, refund_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(cuenta)
    .bind(tipo)
    .bind(monto)
    .bind(saldo)
    .bind(descripcion.chars().take(160).collect::<String>())
    .bind(contraparte.chars().take(160).collect::<String>())
    .bind(pago)
    .bind(devolucion)
    .execute(tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn validar(
        vigencia: Option<i64>,
        vence: Option<DateTime<Utc>>,
    ) -> Result<DatosCobro, ErrorBanco> {
        DatosCobro::validar(
            Some("ref-1".into()),
            Decimal::from_str("350").unwrap(),
            Some("BOB"),
            " Entrada ",
            vigencia,
            vence,
            None,
        )
    }

    #[test]
    fn por_defecto_vence_en_diez_minutos() {
        let datos = validar(None, None).unwrap();
        let segundos = (datos.vence_en - Utc::now()).num_seconds();
        assert!((595..=600).contains(&segundos));
        assert_eq!(datos.descripcion, "Entrada");
        assert_eq!(datos.monto.to_string(), "350.00");
    }

    #[test]
    fn la_vigencia_debe_estar_entre_30_s_y_24_h() {
        assert!(validar(Some(29), None).is_err());
        assert!(validar(Some(30), None).is_ok());
        assert!(validar(Some(86_401), None).is_err());
        assert!(validar(None, Some(Utc::now() + Duration::seconds(10))).is_err());
        assert!(validar(None, Some(Utc::now() + Duration::minutes(5))).is_ok());
        assert!(validar(Some(60), Some(Utc::now() + Duration::minutes(5))).is_err());
    }

    #[test]
    fn la_metadata_debe_ser_un_objeto() {
        let con =
            |m: Value| DatosCobro::validar(None, Decimal::ONE, None, "x", None, None, Some(m));
        assert!(con(serde_json::json!({"ordenId": "abc"})).is_ok());
        assert!(con(serde_json::json!(["abc"])).is_err());
    }

    #[test]
    fn el_payload_y_los_enlaces_usan_la_base() {
        let id = Uuid::nil();
        assert_eq!(
            payload("https://x.trycloudflare.com", id),
            "https://x.trycloudflare.com/?p=00000000-0000-0000-0000-000000000000"
        );
        assert!(enlaces("http://b", id).events.ends_with("/events"));
    }
}
