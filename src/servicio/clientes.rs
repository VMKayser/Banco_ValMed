//! Clientes, cuentas, sesiones de la app, PIN, movimientos y depósitos simulados.

use anyhow::anyhow;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::vistas::{Cuenta, Movimiento, Perfil, SesionCreada};
use super::Banco;
use crate::dominio::{ci, pin, Ci};
use crate::error::ErrorBanco;
use crate::infra::seguridad;

/// Una sesión vence tras este tiempo sin uso…
pub const MINUTOS_INACTIVIDAD: i64 = 15;
/// …o tras este tiempo en total.
pub const HORAS_SESION: i64 = 2;

/// Quien está usando la app (sale del token de sesión).
#[derive(Debug, Clone, Copy)]
pub struct ClienteActual {
    pub sesion_id: Uuid,
    pub cliente_id: Uuid,
    pub cuenta_id: Uuid,
}

pub struct NuevoCliente<'a> {
    pub ci: &'a Ci,
    pub nombre: &'a str,
    pub pin: &'a str,
    pub saldo_inicial: Decimal,
}

pub(crate) fn validar_nombre(nombre: &str) -> Result<String, ErrorBanco> {
    let nombre = nombre.split_whitespace().collect::<Vec<_>>().join(" ");
    let largo = nombre.chars().count();
    if !(3..=120).contains(&largo) {
        return Err(ErrorBanco::validacion(
            "fullName",
            "entre 3 y 120 caracteres",
        ));
    }
    Ok(nombre)
}

impl Banco {
    /// Abre cliente y cuenta en una transacción. El saldo inicial queda como
    /// movimiento `APERTURA`.
    pub async fn abrir_cuenta(&self, nuevo: NuevoCliente<'_>) -> Result<Uuid, ErrorBanco> {
        let nombre = validar_nombre(nuevo.nombre)?;
        pin::validar("pin", nuevo.pin)?;
        let pin_texto = nuevo.pin.to_owned();
        let pin_hash = tokio::task::spawn_blocking(move || seguridad::hashear_pin(&pin_texto))
            .await
            .map_err(|e| anyhow!(e))?
            .map_err(|e| anyhow!("argon2: {e}"))?;

        let mut tx = self.pool().begin().await?;
        let cliente_id = Uuid::new_v4();
        let insertado = sqlx::query(
            "INSERT INTO customers (id, ci_number, ci_complement, full_name, pin_hash)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(cliente_id)
        .bind(nuevo.ci.numero())
        .bind(nuevo.ci.complemento())
        .bind(&nombre)
        .bind(&pin_hash)
        .execute(&mut *tx)
        .await;
        match insertado {
            Err(e) if ErrorBanco::es_unico(&e, "uk_customers_ci") => {
                return Err(ErrorBanco::CiYaRegistrado)
            }
            otro => otro?,
        };

        let cuenta_id = Uuid::new_v4();
        let saldo = nuevo.saldo_inicial.round_dp(2);
        sqlx::query(
            "INSERT INTO accounts (id, customer_id, number, balance)
             VALUES ($1, $2, '1001-' || lpad(nextval('account_number_seq')::text, 6, '0'), $3)",
        )
        .bind(cuenta_id)
        .bind(cliente_id)
        .bind(saldo)
        .execute(&mut *tx)
        .await?;

        if saldo > Decimal::ZERO {
            sqlx::query(
                "INSERT INTO ledger_entries (account_id, kind, amount, balance_after, description)
                 VALUES ($1, 'APERTURA', $2, $2, 'Saldo inicial de la cuenta simulada')",
            )
            .bind(cuenta_id)
            .bind(saldo)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        tracing::info!(ci = %ci::enmascarar(nuevo.ci.numero()), "cuenta abierta");
        Ok(cliente_id)
    }

    pub async fn cliente_por_ci(&self, ci: &Ci) -> Result<Option<Uuid>, ErrorBanco> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM customers WHERE ci_number = $1 AND ci_complement IS NOT DISTINCT FROM $2",
        )
        .bind(ci.numero())
        .bind(ci.complemento())
        .fetch_optional(self.pool())
        .await?)
    }

    /// Entrar con C.I. y PIN. Si el C.I. no existe se verifica igual contra un
    /// hash ficticio, para no revelar qué C.I. tienen cuenta.
    pub async fn iniciar_sesion(
        &self,
        ci: &Ci,
        pin_texto: &str,
    ) -> Result<SesionCreada, ErrorBanco> {
        let Some(cliente_id) = self.cliente_por_ci(ci).await? else {
            let pin_texto = pin_texto.to_owned();
            let _ = tokio::task::spawn_blocking(move || {
                seguridad::verificar_pin(&pin_texto, &seguridad::HASH_FICTICIO)
            })
            .await;
            return Err(ErrorBanco::CredencialesInvalidas);
        };
        self.verificar_pin(cliente_id, pin_texto, true).await?;
        self.crear_sesion(cliente_id).await
    }

    pub(crate) async fn crear_sesion(&self, cliente_id: Uuid) -> Result<SesionCreada, ErrorBanco> {
        let token = seguridad::token("vms_");
        let expira = Utc::now() + Duration::hours(HORAS_SESION);
        sqlx::query(
            "INSERT INTO sessions (id, customer_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(cliente_id)
        .bind(seguridad::sha256(&token))
        .bind(expira)
        .execute(self.pool())
        .await?;
        Ok(SesionCreada {
            token,
            expires_at: expira,
            profile: self.perfil(cliente_id).await?,
        })
    }

    /// Valida el token y renueva la inactividad.
    pub async fn sesion(&self, token: &str) -> Result<Option<ClienteActual>, ErrorBanco> {
        let fila: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
            "UPDATE sessions s SET last_seen_at = now()
             FROM accounts a
             WHERE s.token_hash = $1
               AND s.revoked_at IS NULL
               AND s.expires_at > now()
               AND s.last_seen_at > now() - make_interval(mins => $2)
               AND a.customer_id = s.customer_id
             RETURNING s.id, s.customer_id, a.id",
        )
        .bind(seguridad::sha256(token))
        .bind(MINUTOS_INACTIVIDAD as i32)
        .fetch_optional(self.pool())
        .await?;
        Ok(
            fila.map(|(sesion_id, cliente_id, cuenta_id)| ClienteActual {
                sesion_id,
                cliente_id,
                cuenta_id,
            }),
        )
    }

    pub async fn cerrar_sesion(&self, sesion_id: Uuid) -> Result<(), ErrorBanco> {
        sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(sesion_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Verifica el PIN y lleva la cuenta de fallos. En el login un fallo se
    /// responde como credenciales inválidas; al pagar, con los intentos que quedan.
    pub(crate) async fn verificar_pin(
        &self,
        cliente_id: Uuid,
        pin_texto: &str,
        en_login: bool,
    ) -> Result<(), ErrorBanco> {
        if !en_login {
            pin::validar("pin", pin_texto)?;
        }
        let (hash, bloqueado): (String, Option<DateTime<Utc>>) =
            sqlx::query_as("SELECT pin_hash, locked_until FROM customers WHERE id = $1")
                .bind(cliente_id)
                .fetch_optional(self.pool())
                .await?
                .ok_or(ErrorBanco::NoAutenticado("la sesión ya no es válida"))?;

        if let Some(hasta) = bloqueado.filter(|h| *h > Utc::now()) {
            return Err(ErrorBanco::CuentaBloqueada {
                segundos: (hasta - Utc::now()).num_seconds().max(1),
            });
        }

        let pin_copia = pin_texto.to_owned();
        let correcto =
            tokio::task::spawn_blocking(move || seguridad::verificar_pin(&pin_copia, &hash))
                .await
                .map_err(|e| anyhow!(e))?;

        if correcto {
            sqlx::query(
                "UPDATE customers SET failed_pin_attempts = 0, locked_until = NULL
                 WHERE id = $1 AND (failed_pin_attempts <> 0 OR locked_until IS NOT NULL)",
            )
            .bind(cliente_id)
            .execute(self.pool())
            .await?;
            return Ok(());
        }

        // En el SET, las columnas valen lo de antes del UPDATE.
        let (fallidos, bloqueado): (i32, Option<DateTime<Utc>>) = sqlx::query_as(
            "UPDATE customers SET
                 failed_pin_attempts = CASE WHEN failed_pin_attempts + 1 >= $2 THEN 0
                                            ELSE failed_pin_attempts + 1 END,
                 locked_until = CASE WHEN failed_pin_attempts + 1 >= $2
                                     THEN now() + make_interval(mins => $3)
                                     ELSE NULL END
             WHERE id = $1
             RETURNING failed_pin_attempts, locked_until",
        )
        .bind(cliente_id)
        .bind(pin::INTENTOS_MAXIMOS)
        .bind(pin::MINUTOS_BLOQUEO)
        .fetch_one(self.pool())
        .await?;

        if let Some(hasta) = bloqueado {
            tracing::warn!(%cliente_id, "cuenta bloqueada por PIN incorrectos");
            return Err(ErrorBanco::CuentaBloqueada {
                segundos: (hasta - Utc::now()).num_seconds().max(1),
            });
        }
        if en_login {
            Err(ErrorBanco::CredencialesInvalidas)
        } else {
            Err(ErrorBanco::PinIncorrecto {
                restantes: pin::INTENTOS_MAXIMOS - fallidos,
            })
        }
    }

    pub async fn perfil(&self, cliente_id: Uuid) -> Result<Perfil, ErrorBanco> {
        let fila: Option<(String, String, Option<String>, String, Decimal, String)> =
            sqlx::query_as(
                "SELECT c.full_name, c.ci_number, c.ci_complement, a.number, a.balance, a.currency
                 FROM customers c JOIN accounts a ON a.customer_id = c.id
                 WHERE c.id = $1",
            )
            .bind(cliente_id)
            .fetch_optional(self.pool())
            .await?;
        let (nombre, numero_ci, complemento, numero, saldo, moneda) =
            fila.ok_or(ErrorBanco::NoAutenticado("la sesión ya no es válida"))?;
        Ok(Perfil {
            customer_id: cliente_id,
            full_name: nombre,
            ci: ci::formatear(&numero_ci, complemento.as_deref()),
            account: Cuenta {
                number: numero,
                balance: saldo,
                currency: moneda,
            },
        })
    }

    /// Del más reciente al más antiguo; `antes_de` es el `id` del último que ya se mostró.
    pub async fn movimientos(
        &self,
        cuenta_id: Uuid,
        limite: i64,
        antes_de: Option<i64>,
    ) -> Result<Vec<Movimiento>, ErrorBanco> {
        type Fila = (
            i64,
            String,
            Decimal,
            Decimal,
            String,
            Option<String>,
            Option<String>,
            DateTime<Utc>,
        );
        let filas: Vec<Fila> = sqlx::query_as(
            "SELECT l.id, l.kind, l.amount, l.balance_after, l.description, l.counterparty,
                    p.bank_reference, l.created_at
             FROM ledger_entries l LEFT JOIN payments p ON p.id = l.payment_id
             WHERE l.account_id = $1 AND ($2::bigint IS NULL OR l.id < $2)
             ORDER BY l.id DESC
             LIMIT $3",
        )
        .bind(cuenta_id)
        .bind(antes_de)
        .bind(limite.clamp(1, 100))
        .fetch_all(self.pool())
        .await?;
        Ok(filas
            .into_iter()
            .map(
                |(
                    id,
                    kind,
                    amount,
                    balance_after,
                    description,
                    counterparty,
                    bank_reference,
                    created_at,
                )| {
                    Movimiento {
                        id,
                        kind,
                        amount,
                        balance_after,
                        description,
                        counterparty,
                        bank_reference,
                        created_at,
                    }
                },
            )
            .collect())
    }

    /// Depósito simulado: aparece dinero en la cuenta (para probar saldo insuficiente).
    pub async fn depositar(&self, cuenta_id: Uuid, monto: Decimal) -> Result<Cuenta, ErrorBanco> {
        let mut tx = self.pool().begin().await?;
        let (numero, saldo, moneda): (String, Decimal, String) = sqlx::query_as(
            "UPDATE accounts SET balance = balance + $2 WHERE id = $1
             RETURNING number, balance, currency",
        )
        .bind(cuenta_id)
        .bind(monto)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO ledger_entries (account_id, kind, amount, balance_after, description)
             VALUES ($1, 'DEPOSITO', $2, $3, 'Depósito simulado')",
        )
        .bind(cuenta_id)
        .bind(monto)
        .bind(saldo)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Cuenta {
            number: numero,
            balance: saldo,
            currency: moneda,
        })
    }
}
