//! Datos de demostración. Se pueden correr en cada arranque: lo que ya existe se
//! deja como está.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rust_decimal::Decimal;

use super::clientes::NuevoCliente;
use super::Banco;
use crate::dominio::Ci;
use crate::error::ErrorBanco;
use crate::infra::seguridad;

pub const PIN_DEMO: &str = "123456";

/// (C.I., nombre, saldo inicial en centavos)
const CLIENTES_DEMO: [(&str, &str, i64); 4] = [
    ("7000001", "Rosa Quispe", 200_000),
    ("7000002", "Juan Mamani", 5_000),
    ("7000003", "Luis Choque", 100_000),
    ("7000004", "Carla Flores", 100_000),
];

/// Dueño de la clave de API fija (`BANK_SEED_API_KEY`).
const COMERCIO_DEMO: (&str, &str) = ("9000001", "Comercio Demo");

pub async fn sembrar(banco: &Banco) -> anyhow::Result<()> {
    let config = banco.config().clone();
    if config.semilla_demo {
        for (ci, nombre, centavos) in CLIENTES_DEMO {
            asegurar_cliente(
                banco,
                &Ci::desde_texto(ci)?,
                nombre,
                Decimal::new(centavos, 2),
            )
            .await?;
        }
        tracing::info!(
            clientes = CLIENTES_DEMO.len(),
            "clientes de demostración listos"
        );
    }

    if let Some(semilla) = &config.semilla_clave {
        let (ci, nombre) = COMERCIO_DEMO;
        let cliente_id =
            asegurar_cliente(banco, &Ci::desde_texto(ci)?, nombre, Decimal::ZERO).await?;
        let clave = banco
            .guardar_clave(
                cliente_id,
                "Clave de demostración",
                &semilla.api_key,
                &account_id_semilla(&semilla.api_key),
                semilla.webhook_url.clone(),
                &semilla.webhook_secret,
            )
            .await?;
        tracing::info!(
            webhook = semilla.webhook_url.is_some(),
            account_id = %clave.account_id,
            "clave de API de demostración lista (authorizationId = BANK_SEED_API_KEY)"
        );
    }
    Ok(())
}

/// `accountId` de la clave fija: sale de la clave, así es el mismo aunque se borre
/// la base y no hace falta otra variable de entorno.
pub fn account_id_semilla(api_key: &str) -> String {
    STANDARD.encode(&seguridad::sha256(&format!("api-market:{api_key}"))[..16])
}

/// Devuelve el cliente con ese C.I.; si no existe, le abre una cuenta con PIN
/// 123456 (también lo usa el sandbox). Tolera que otra petición lo cree a la vez.
pub(crate) async fn asegurar_cliente(
    banco: &Banco,
    ci: &Ci,
    nombre: &str,
    saldo: Decimal,
) -> Result<uuid::Uuid, ErrorBanco> {
    if let Some(id) = banco.cliente_por_ci(ci).await? {
        return Ok(id);
    }
    match banco
        .abrir_cuenta(NuevoCliente {
            ci,
            nombre,
            pin: PIN_DEMO,
            saldo_inicial: saldo,
        })
        .await
    {
        Err(ErrorBanco::CiYaRegistrado) => banco
            .cliente_por_ci(ci)
            .await?
            .ok_or(ErrorBanco::CiYaRegistrado),
        otro => otro,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_account_id_de_la_semilla_es_estable() {
        let a = account_id_semilla("vm_test_fija");
        assert_eq!(a, account_id_semilla("vm_test_fija"));
        assert_ne!(a, account_id_semilla("vm_test_otra"));
        assert_eq!(a.len(), 24);
    }
}
