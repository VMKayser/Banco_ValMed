//! Montos en bolivianos con 2 decimales. En JSON viajan como string (`"350.00"`)
//! para que ningún cliente los pase por `float`.

use rust_decimal::Decimal;

use crate::error::ErrorBanco;

pub const MONEDA: &str = "BOB";

/// Tope de un cobro QR.
pub fn maximo_cobro() -> Decimal {
    Decimal::new(100_000, 0)
}

/// Tope de un depósito simulado.
pub fn maximo_deposito() -> Decimal {
    Decimal::new(5_000, 0)
}

/// Valida que sea positivo, con 2 decimales como máximo y bajo el tope; lo
/// devuelve con escala 2 (`350` → `350.00`).
pub fn monto(campo: &str, valor: Decimal, maximo: Decimal) -> Result<Decimal, ErrorBanco> {
    let normal = valor.normalize();
    if normal.scale() > 2 {
        return Err(ErrorBanco::validacion(campo, "máximo 2 decimales"));
    }
    if normal <= Decimal::ZERO {
        return Err(ErrorBanco::validacion(campo, "debe ser mayor que 0"));
    }
    if normal > maximo {
        return Err(ErrorBanco::validacion(campo, format!("máximo {maximo}")));
    }
    let mut con_escala = normal;
    con_escala.rescale(2);
    Ok(con_escala)
}

/// Solo existe una moneda; si el cliente la manda, debe ser BOB.
pub fn moneda(valor: Option<&str>) -> Result<(), ErrorBanco> {
    match valor.map(str::trim) {
        None | Some("") => Ok(()),
        Some(m) if m.eq_ignore_ascii_case(MONEDA) => Ok(()),
        Some(_) => Err(ErrorBanco::validacion("currency", "solo BOB")),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn d(texto: &str) -> Decimal {
        Decimal::from_str(texto).unwrap()
    }

    #[test]
    fn deja_el_monto_con_dos_decimales() {
        assert_eq!(
            monto("amount", d("350"), maximo_cobro())
                .unwrap()
                .to_string(),
            "350.00"
        );
        assert_eq!(
            monto("amount", d("0.5"), maximo_cobro())
                .unwrap()
                .to_string(),
            "0.50"
        );
        assert_eq!(
            monto("amount", d("12.300"), maximo_cobro())
                .unwrap()
                .to_string(),
            "12.30"
        );
    }

    #[test]
    fn rechaza_montos_invalidos() {
        assert!(monto("amount", d("0"), maximo_cobro()).is_err());
        assert!(monto("amount", d("-1"), maximo_cobro()).is_err());
        assert!(monto("amount", d("1.234"), maximo_cobro()).is_err());
        assert!(monto("amount", d("100000.01"), maximo_cobro()).is_err());
        assert!(monto("amount", d("5000.01"), maximo_deposito()).is_err());
    }

    #[test]
    fn solo_acepta_bolivianos() {
        assert!(moneda(None).is_ok());
        assert!(moneda(Some("bob")).is_ok());
        assert!(moneda(Some("USD")).is_err());
    }
}
