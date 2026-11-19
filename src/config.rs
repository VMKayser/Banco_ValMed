//! Configuración por variables de entorno (ver `.env.example`).

use std::str::FromStr;

use anyhow::{bail, Context, Result};
use rust_decimal::Decimal;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub puerto_http: u16,
    /// URL pública del banco (la que abre el celular). Si falta, se arma con las
    /// cabeceras de cada petición (`Host`, `X-Forwarded-*`).
    pub url_publica: Option<String>,
    /// Puerto publicado en el host (`BANK_HOST_PORT`): si una petición llega por un
    /// nombre interno de Docker (`banco:8080`), los QR usan `http://localhost:<este>`.
    pub puerto_host: Option<u16>,
    pub carpeta_estatica: String,
    /// Habilita `POST /api/v1/sandbox/qr/{id}/pay` (pagar sin la app; `id` puede
    /// ser el UUID o el `qrId` numérico de la API Market).
    pub sandbox: bool,
    pub saldo_inicial: Decimal,
    pub semilla_demo: bool,
    pub semilla_clave: Option<SemillaClave>,
    /// Firma los tokens de la API Market (`BANK_TOKEN_SECRET`). Si falta, se
    /// genera uno al arrancar y los tokens dejan de servir al reiniciar.
    pub secreto_tokens: SecretoTokens,
}

/// No se imprime en los logs.
#[derive(Clone)]
pub struct SecretoTokens {
    bytes: Vec<u8>,
    pub fijo: bool,
}

impl SecretoTokens {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::fmt::Debug for SecretoTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretoTokens(***)")
    }
}

/// Clave de API fija para que otros equipos la pongan en su `.env`.
#[derive(Clone, Debug)]
pub struct SemillaClave {
    pub api_key: String,
    pub webhook_url: Option<String>,
    pub webhook_secret: String,
}

fn variable(nombre: &str) -> Option<String> {
    std::env::var(nombre)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn booleano(nombre: &str) -> bool {
    matches!(
        variable(nombre)
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("true" | "1" | "yes" | "si" | "sí")
    )
}

pub fn puerto_desde_entorno() -> Result<u16> {
    variable("BANK_HTTP_PORT")
        .map(|p| p.parse::<u16>())
        .transpose()
        .context("BANK_HTTP_PORT inválido")
        .map(|p| p.unwrap_or(8080))
}

impl Config {
    pub fn desde_entorno() -> Result<Self> {
        let database_url = variable("DATABASE_URL").context("falta DATABASE_URL")?;
        let saldo_inicial = variable("BANK_INITIAL_BALANCE")
            .map(|s| Decimal::from_str(&s))
            .transpose()
            .context("BANK_INITIAL_BALANCE inválido")?
            .unwrap_or(Decimal::new(100_000, 2));
        if saldo_inicial.is_sign_negative() {
            bail!("BANK_INITIAL_BALANCE no puede ser negativo");
        }

        let semilla_clave = match variable("BANK_SEED_API_KEY") {
            None => None,
            Some(api_key) => Some(SemillaClave {
                api_key,
                webhook_url: variable("BANK_SEED_WEBHOOK_URL"),
                webhook_secret: variable("BANK_SEED_WEBHOOK_SECRET")
                    .context("BANK_SEED_API_KEY necesita BANK_SEED_WEBHOOK_SECRET")?,
            }),
        };

        let secreto_tokens = match variable("BANK_TOKEN_SECRET") {
            Some(s) if s.len() < 32 => bail!("BANK_TOKEN_SECRET debe tener al menos 32 caracteres"),
            Some(s) => SecretoTokens {
                bytes: s.into_bytes(),
                fijo: true,
            },
            None => {
                let mut bytes = vec![0u8; 32];
                rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
                SecretoTokens { bytes, fijo: false }
            }
        };

        Ok(Self {
            database_url,
            puerto_http: puerto_desde_entorno()?,
            url_publica: variable("BANK_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_owned()),
            puerto_host: variable("BANK_HOST_PORT").and_then(|p| p.parse().ok()),
            carpeta_estatica: variable("BANK_STATIC_DIR").unwrap_or_else(|| "static".to_owned()),
            sandbox: booleano("BANK_SANDBOX"),
            saldo_inicial,
            semilla_demo: booleano("BANK_SEED_DEMO"),
            semilla_clave,
            secreto_tokens,
        })
    }
}
