//! Cédula de identidad boliviana, con las mismas reglas que Identity de SmartFila:
//! de 5 a 10 dígitos y un complemento opcional de 2 caracteres (ej. `1234567-1A`).
//! Sin verificación externa: el banco confía en lo que escribe el cliente.

use std::fmt;

use crate::error::{Detalle, ErrorBanco};

const DIGITOS_MIN: usize = 5;
const DIGITOS_MAX: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ci {
    numero: String,
    complemento: Option<String>,
}

impl Ci {
    /// Valida y normaliza. Quita espacios; un complemento vacío cuenta como ausente.
    pub fn nuevo(numero: &str, complemento: Option<&str>) -> Result<Self, ErrorBanco> {
        let mut errores = Vec::new();
        let numero = numero.trim();
        if numero.is_empty() {
            errores.push(Detalle::new("ci", "obligatorio"));
        } else if !numero.chars().all(|c| c.is_ascii_digit()) {
            errores.push(Detalle::new("ci", "solo dígitos"));
        } else if !(DIGITOS_MIN..=DIGITOS_MAX).contains(&numero.len()) {
            errores.push(Detalle::new(
                "ci",
                format!("entre {DIGITOS_MIN} y {DIGITOS_MAX} dígitos"),
            ));
        }

        let complemento = complemento
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_ascii_uppercase);
        if let Some(c) = &complemento {
            if c.len() != 2 || !c.chars().all(|ch| ch.is_ascii_alphanumeric()) {
                errores.push(Detalle::new("ciComplement", "2 letras o dígitos"));
            }
        }

        if errores.is_empty() {
            Ok(Self {
                numero: numero.to_owned(),
                complemento,
            })
        } else {
            Err(ErrorBanco::Validacion(errores))
        }
    }

    /// Acepta la forma canónica `1234567` o `1234567-1A`.
    pub fn desde_texto(texto: &str) -> Result<Self, ErrorBanco> {
        match texto.trim().split_once('-') {
            Some((numero, complemento)) => Self::nuevo(numero, Some(complemento)),
            None => Self::nuevo(texto, None),
        }
    }

    pub fn numero(&self) -> &str {
        &self.numero
    }

    pub fn complemento(&self) -> Option<&str> {
        self.complemento.as_deref()
    }
}

impl fmt::Display for Ci {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&formatear(&self.numero, self.complemento.as_deref()))
    }
}

/// Forma canónica a partir de las columnas de la base.
pub fn formatear(numero: &str, complemento: Option<&str>) -> String {
    match complemento {
        Some(c) => format!("{numero}-{c}"),
        None => numero.to_owned(),
    }
}

/// `7000001` → `7000***`, para logs y vistas públicas.
pub fn enmascarar(numero: &str) -> String {
    let visibles = numero.len().saturating_sub(3).max(1).min(numero.len());
    format!(
        "{}{}",
        &numero[..visibles],
        "*".repeat(numero.len() - visibles)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acepta_ci_con_y_sin_complemento() {
        assert_eq!(Ci::nuevo("7000001", None).unwrap().to_string(), "7000001");
        assert_eq!(
            Ci::nuevo(" 1234567 ", Some("1a")).unwrap().to_string(),
            "1234567-1A"
        );
        assert_eq!(
            Ci::desde_texto("1234567-1A").unwrap(),
            Ci::nuevo("1234567", Some("1A")).unwrap()
        );
    }

    #[test]
    fn un_complemento_vacio_cuenta_como_ausente() {
        assert_eq!(
            Ci::nuevo("7000001", Some("  ")).unwrap().complemento(),
            None
        );
    }

    #[test]
    fn rechaza_ci_invalidos() {
        for malo in ["", "12a45", "1234", "12345678901"] {
            assert!(Ci::nuevo(malo, None).is_err(), "{malo} debería fallar");
        }
        assert!(Ci::nuevo("1234567", Some("1AB")).is_err());
        assert!(Ci::nuevo("1234567", Some("1-")).is_err());
    }

    #[test]
    fn enmascara_los_ultimos_tres_digitos() {
        assert_eq!(enmascarar("7000001"), "7000***");
        assert_eq!(enmascarar("12345"), "12***");
    }
}
