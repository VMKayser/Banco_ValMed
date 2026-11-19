//! PIN de 6 dígitos. Se pide al entrar y otra vez al confirmar cada pago.
//! Tras `INTENTOS_MAXIMOS` fallos seguidos la cuenta se bloquea `MINUTOS_BLOQUEO`.

use crate::error::ErrorBanco;

pub const INTENTOS_MAXIMOS: i32 = 5;
pub const MINUTOS_BLOQUEO: i32 = 5;

pub fn validar(campo: &str, pin: &str) -> Result<(), ErrorBanco> {
    if pin.len() == 6 && pin.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(ErrorBanco::validacion(campo, "exactamente 6 dígitos"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_seis_digitos() {
        assert!(validar("pin", "123456").is_ok());
        for malo in ["", "12345", "1234567", "12345a", "١٢٣٤٥٦"] {
            assert!(validar("pin", malo).is_err(), "{malo} debería fallar");
        }
    }
}
