//! Firma de los avisos (webhooks): `sha256=` + hex(HMAC-SHA256(secreto,
//! `timestamp` + "." + cuerpo)). El integrador la recalcula sobre los bytes crudos
//! del cuerpo y rechaza si el timestamp tiene más de 5 minutos.

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub fn firmar(secreto: &str, timestamp: i64, cuerpo: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secreto.as_bytes())
        .expect("HMAC acepta claves de cualquier largo");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(cuerpo.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El mismo vector está en la guía de integración: si un integrador obtiene
    /// este valor, firma igual que el banco.
    #[test]
    fn coincide_con_el_vector_de_la_guia() {
        assert_eq!(
            firmar("whsec_prueba", 1_763_490_725, r#"{"id":"evt_1"}"#),
            "sha256=0a39d4ae3eb66f9599fc86e329b9352704d02a4ba76e8dba30c699b9f8022568"
        );
    }

    #[test]
    fn cambia_si_cambia_cualquier_parte() {
        let base = firmar("s", 1, "{}");
        assert_ne!(base, firmar("t", 1, "{}"));
        assert_ne!(base, firmar("s", 2, "{}"));
        assert_ne!(base, firmar("s", 1, "{ }"));
    }
}
