//! Código corto de un cobro QR, para pagar sin cámara: 8 caracteres Crockford
//! base32 (sin I, L, O ni U), mostrado como `7K2Q-M9XD`. 40 bits: no sirve como
//! secreto, por eso solo resuelve cobros activos y la búsqueda pide sesión.

use rand::Rng;

const ALFABETO: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const LARGO: usize = 8;

pub fn generar() -> String {
    let mut rng = rand::rng();
    (0..LARGO)
        .map(|_| ALFABETO[rng.random_range(0..ALFABETO.len())] as char)
        .collect()
}

/// Código de transacción de 10 caracteres (el `VoucherId` del BNB, ej. `3JAO702813`).
pub fn voucher() -> String {
    const ALFANUMERICO: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut rng = rand::rng();
    (0..10)
        .map(|_| ALFANUMERICO[rng.random_range(0..ALFANUMERICO.len())] as char)
        .collect()
}

/// Acepta minúsculas, guiones y espacios, y corrige I/L → 1 y O → 0.
pub fn normalizar(entrada: &str) -> Option<String> {
    let limpio: String = entrada
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            otro => otro,
        })
        .collect();
    (limpio.len() == LARGO && limpio.bytes().all(|b| ALFABETO.contains(&b))).then_some(limpio)
}

/// `7K2QM9XD` → `7K2Q-M9XD`.
pub fn formatear(codigo: &str) -> String {
    let codigo = codigo.trim();
    if codigo.len() == LARGO && codigo.is_ascii() {
        format!("{}-{}", &codigo[..4], &codigo[4..])
    } else {
        codigo.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genera_codigos_validos() {
        for _ in 0..200 {
            let c = generar();
            assert_eq!(normalizar(&c).as_deref(), Some(c.as_str()));
        }
    }

    #[test]
    fn normaliza_lo_que_escribe_una_persona() {
        assert_eq!(normalizar("7k2q-m9xd").as_deref(), Some("7K2QM9XD"));
        assert_eq!(normalizar("OIL2 3456").as_deref(), Some("01123456"));
        assert_eq!(normalizar("7K2Q-M9X"), None);
        assert_eq!(normalizar("7K2Q-M9XU"), None);
    }

    #[test]
    fn el_voucher_tiene_10_alfanumericos_en_mayuscula() {
        let v = voucher();
        assert_eq!(v.len(), 10);
        assert!(v
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase()));
        assert_ne!(v, voucher());
    }

    #[test]
    fn formatea_con_guion() {
        assert_eq!(formatear("7K2QM9XD"), "7K2Q-M9XD");
    }
}
