//! Reglas de la API Market (Pago QR) del BNB que imita Banco ValMed: fechas en
//! hora de Bolivia sin zona, estados numéricos, vencimiento por día y lectura
//! tolerante de los campos (el BNB acepta `"amount": 20` y `"amount": "20"`).

use std::collections::HashMap;
use std::str::FromStr;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::servicio::vistas::EstadoCobro;

/// Bolivia: UTC-4 todo el año (sin horario de verano).
pub fn hora_bolivia() -> FixedOffset {
    FixedOffset::west_opt(4 * 3600).expect("UTC-4 es un desfase válido")
}

/// Lo que devuelve el BNB cuando un QR no se usó (`DateTime.MinValue` de .NET).
pub const FECHA_SIN_USO: &str = "0001-01-01T00:00:00";

/// Id de banco que va en `sourceBankId`: todos los pagos salen de Banco ValMed.
pub const ID_BANCO_VALMED: i32 = 1;
pub const NOMBRE_BANCO: &str = "BANCO VALMED";

/// `destinationAccountId` 1 = cuenta en bolivianos (la única que existe aquí).
pub const CUENTA_DESTINO_BOB: i64 = 1;

/// Máximo de días hacia adelante para `expirationDate`.
pub const DIAS_MAXIMOS: i64 = 365;

/// Estado del QR como lo numera el BNB: 1 no usado, 2 usado, 3 expirado, 4 con
/// error. El BNB no documenta qué devuelve tras anular; aquí es 4.
pub fn estado_id(estado: EstadoCobro) -> i32 {
    match estado {
        EstadoCobro::Active => 1,
        EstadoCobro::Paid | EstadoCobro::Refunded => 2,
        EstadoCobro::Expired => 3,
        EstadoCobro::Cancelled => 4,
    }
}

/// `2026-11-18T15:30:00` en hora de Bolivia, sin zona (como el BNB).
pub fn fecha_local(fecha: DateTime<Utc>) -> String {
    fecha
        .with_timezone(&hora_bolivia())
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// `18/11/2026 15:30:00`: formato de `TransactionDateTime` en la notificación.
pub fn fecha_notificacion(fecha: DateTime<Utc>) -> String {
    fecha
        .with_timezone(&hora_bolivia())
        .format("%d/%m/%Y %H:%M:%S")
        .to_string()
}

/// Día de hoy en Bolivia.
pub fn hoy(ahora: DateTime<Utc>) -> NaiveDate {
    ahora.with_timezone(&hora_bolivia()).date_naive()
}

/// `expirationDate` es el último día en que se puede pagar: el QR vence a la
/// medianoche (hora de Bolivia) del día siguiente.
pub fn vencimiento(ultimo_dia: NaiveDate, ahora: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let hoy = hoy(ahora);
    if ultimo_dia < hoy {
        return Err(format!(
            "expirationDate no puede ser anterior a hoy ({hoy})"
        ));
    }
    if ultimo_dia > hoy + Duration::days(DIAS_MAXIMOS) {
        return Err(format!(
            "expirationDate puede estar como máximo {DIAS_MAXIMOS} días adelante"
        ));
    }
    let medianoche = (ultimo_dia + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 existe");
    hora_bolivia()
        .from_local_datetime(&medianoche)
        .single()
        .map(|f| f.with_timezone(&Utc))
        .ok_or_else(|| "expirationDate inválida".to_owned())
}

/// `newAuthorizationId`: 15 caracteres o más, con al menos una letra, un número
/// y un carácter especial de `+ - / * $ < > ! ¡ # | =` (regla del BNB).
pub fn autorizacion_segura(nueva: &str) -> bool {
    const ESPECIALES: &str = "+-/*$<>!¡#|=";
    nueva.chars().count() >= 15
        && nueva.chars().any(char::is_alphabetic)
        && nueva.chars().any(|c| c.is_ascii_digit())
        && nueva.chars().any(|c| ESPECIALES.contains(c))
}

/// Monto como número JSON: `20` si es entero, `20.5` si no (como el BNB).
pub fn numero(monto: Decimal) -> Value {
    let normal = monto.normalize();
    if normal.scale() == 0 {
        if let Some(entero) = normal.to_i64() {
            return Value::from(entero);
        }
    }
    normal.to_f64().map(Value::from).unwrap_or(Value::Null)
}

/// Campos del cuerpo con la clave en minúsculas: el BNB (ASP.NET) no distingue
/// `AccountId` de `accountId`, y su propia documentación usa los dos.
#[derive(Debug, Default)]
pub struct Campos(HashMap<String, Value>);

impl Campos {
    pub fn desde_json(valor: Value) -> Option<Self> {
        match valor {
            Value::Object(mapa) => Some(Self(
                mapa.into_iter()
                    .map(|(k, v)| (k.to_lowercase(), v))
                    .collect(),
            )),
            _ => None,
        }
    }

    fn valor(&self, nombre: &str) -> Option<&Value> {
        self.0.get(&nombre.to_lowercase()).filter(|v| !v.is_null())
    }

    /// Texto obligatorio (sin espacios a los lados, no vacío).
    pub fn texto(&self, nombre: &str) -> Result<String, String> {
        self.texto_opcional(nombre)?
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("{nombre} es obligatorio"))
    }

    pub fn texto_opcional(&self, nombre: &str) -> Result<Option<String>, String> {
        match self.valor(nombre) {
            None => Ok(None),
            Some(Value::String(t)) => Ok(Some(t.trim().to_owned())),
            Some(Value::Number(n)) => Ok(Some(n.to_string())),
            Some(_) => Err(format!("{nombre} debe ser texto")),
        }
    }

    /// Número o texto con un número: `20`, `20.5`, `"20"`.
    pub fn decimal(&self, nombre: &str) -> Result<Decimal, String> {
        let error = || format!("{nombre} debe ser un número");
        let texto = match self.valor(nombre) {
            None => return Err(format!("{nombre} es obligatorio")),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(t)) => t.trim().to_owned(),
            Some(_) => return Err(error()),
        };
        Decimal::from_str(&texto)
            .or_else(|_| Decimal::from_scientific(&texto))
            .map_err(|_| error())
    }

    /// Entero o texto con un entero: `59`, `"59"`.
    pub fn entero(&self, nombre: &str) -> Result<Option<i64>, String> {
        match self.valor(nombre) {
            None => Ok(None),
            Some(Value::Number(n)) => n
                .as_i64()
                .map(Some)
                .ok_or_else(|| format!("{nombre} debe ser un entero")),
            Some(Value::String(t)) => t
                .trim()
                .parse()
                .map(Some)
                .map_err(|_| format!("{nombre} debe ser un entero")),
            Some(_) => Err(format!("{nombre} debe ser un entero")),
        }
    }

    /// `true`, `false`, `"true"` o `"false"`.
    pub fn booleano(&self, nombre: &str) -> Result<Option<bool>, String> {
        match self.valor(nombre) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(Value::String(t)) if t.trim().eq_ignore_ascii_case("true") => Ok(Some(true)),
            Some(Value::String(t)) if t.trim().eq_ignore_ascii_case("false") => Ok(Some(false)),
            Some(_) => Err(format!("{nombre} debe ser true o false")),
        }
    }

    /// `2026-11-18` (también acepta `2026-11-18T00:00:00` y se queda con el día).
    pub fn fecha(&self, nombre: &str) -> Result<NaiveDate, String> {
        let texto = self.texto(nombre)?;
        NaiveDate::parse_from_str(&texto, "%Y-%m-%d")
            .or_else(|_| {
                NaiveDateTime::parse_from_str(&texto, "%Y-%m-%dT%H:%M:%S").map(|f| f.date())
            })
            .map_err(|_| format!("{nombre} debe tener el formato AAAA-MM-DD"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn campos(v: Value) -> Campos {
        Campos::desde_json(v).unwrap()
    }

    #[test]
    fn lee_numeros_y_booleanos_como_numero_o_texto() {
        let c = campos(json!({"amount": 20, "Monto": "20.50", "singleUse": "true", "QrId": "59"}));
        assert_eq!(c.decimal("amount").unwrap().to_string(), "20");
        assert_eq!(c.decimal("monto").unwrap().to_string(), "20.50");
        assert_eq!(c.booleano("singleuse").unwrap(), Some(true));
        assert_eq!(c.entero("qrId").unwrap(), Some(59));
        assert!(c.decimal("falta").is_err());
        assert!(campos(json!({"qrId": "abc"})).entero("qrId").is_err());
        assert!(campos(json!({"amount": [1]})).decimal("amount").is_err());
        assert!(Campos::desde_json(json!([1])).is_none());
    }

    #[test]
    fn las_claves_no_distinguen_mayusculas() {
        let c = campos(json!({"AccountId": "abc", "gloss": "  Entrada  "}));
        assert_eq!(c.texto("accountId").unwrap(), "abc");
        assert_eq!(c.texto("Gloss").unwrap(), "Entrada");
        assert!(campos(json!({"gloss": "  "})).texto("gloss").is_err());
    }

    #[test]
    fn el_qr_vence_a_medianoche_de_bolivia_del_dia_siguiente() {
        // 18/11/2026 23:30 en Bolivia = 19/11/2026 03:30 UTC.
        let ahora = Utc.with_ymd_and_hms(2026, 11, 19, 3, 30, 0).unwrap();
        assert_eq!(hoy(ahora), NaiveDate::from_ymd_opt(2026, 11, 18).unwrap());
        let vence = vencimiento(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(), ahora).unwrap();
        assert_eq!(vence, Utc.with_ymd_and_hms(2026, 11, 19, 4, 0, 0).unwrap());
        assert_eq!(fecha_local(vence), "2026-11-19T00:00:00");
        assert!(vencimiento(NaiveDate::from_ymd_opt(2026, 11, 17).unwrap(), ahora).is_err());
        assert!(vencimiento(NaiveDate::from_ymd_opt(2027, 12, 1).unwrap(), ahora).is_err());
    }

    #[test]
    fn formatea_fechas_como_el_bnb() {
        let f = Utc.with_ymd_and_hms(2026, 11, 18, 19, 30, 15).unwrap();
        assert_eq!(fecha_local(f), "2026-11-18T15:30:15");
        assert_eq!(fecha_notificacion(f), "18/11/2026 15:30:15");
        let c = campos(json!({"a": "2026-11-18", "b": "2026-11-18T00:00:00", "c": "18/11/2026"}));
        assert!(c.fecha("a").is_ok() && c.fecha("b").is_ok() && c.fecha("c").is_err());
    }

    #[test]
    fn numera_los_estados_como_el_bnb() {
        assert_eq!(estado_id(EstadoCobro::Active), 1);
        assert_eq!(estado_id(EstadoCobro::Paid), 2);
        assert_eq!(estado_id(EstadoCobro::Refunded), 2);
        assert_eq!(estado_id(EstadoCobro::Expired), 3);
        assert_eq!(estado_id(EstadoCobro::Cancelled), 4);
    }

    #[test]
    fn exige_una_autorizacion_segura() {
        assert!(autorizacion_segura("Naciona0123456*"));
        assert!(!autorizacion_segura("Naciona012345*")); // 14
        assert!(!autorizacion_segura("Nacionaaaaaaaa*")); // sin número
        assert!(!autorizacion_segura("Naciona01234567")); // sin especial
        assert!(!autorizacion_segura("012345678901234*")); // sin letra
    }

    #[test]
    fn el_monto_sale_como_numero() {
        assert_eq!(numero(Decimal::new(2000, 2)).to_string(), "20");
        assert_eq!(numero(Decimal::new(2050, 2)).to_string(), "20.5");
        assert_eq!(numero(Decimal::new(1, 2)).to_string(), "0.01");
    }
}
