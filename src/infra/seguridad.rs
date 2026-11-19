//! PIN con Argon2id y tokens aleatorios. De los tokens (sesiones y claves de API)
//! la base guarda solo el SHA-256: una copia de la base no sirve para entrar.

use std::sync::LazyLock;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Se verifica contra este hash cuando el C.I. no existe, para que el login tarde
/// lo mismo y no revele qué C.I. tienen cuenta.
pub static HASH_FICTICIO: LazyLock<String> =
    LazyLock::new(|| hashear_pin("000000").expect("hash ficticio"));

pub fn hashear_pin(pin: &str) -> Result<String, argon2::password_hash::Error> {
    let sal = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(pin.as_bytes(), &sal)?
        .to_string())
}

/// `false` también si el hash guardado está corrupto.
pub fn verificar_pin(pin: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        Argon2::default()
            .verify_password(pin.as_bytes(), &h)
            .is_ok()
    })
}

/// 256 bits aleatorios en base64url, con un prefijo que dice qué es
/// (`vms_` sesión, `vm_test_` clave de API, `whsec_` secreto de avisos).
pub fn token(prefijo: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefijo}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// `accountId` de la API Market: 16 bytes aleatorios en base64 (24 caracteres,
/// como los que entrega el BNB).
pub fn identificador_market() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    STANDARD.encode(bytes)
}

pub fn sha256(texto: &str) -> Vec<u8> {
    Sha256::digest(texto.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifica_el_pin_correcto() {
        let hash = hashear_pin("123456").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verificar_pin("123456", &hash));
        assert!(!verificar_pin("123457", &hash));
        assert!(!verificar_pin("123456", "no-es-un-hash"));
    }

    #[test]
    fn los_tokens_no_se_repiten_y_llevan_prefijo() {
        let a = token("vms_");
        assert!(a.starts_with("vms_"));
        assert_eq!(a.len(), 4 + 43);
        assert_ne!(a, token("vms_"));
        assert_eq!(sha256(&a).len(), 32);
    }
}
