//! JWT HS256 mínimo para el token de la API Market (el BNB también entrega un
//! JWT en `message`). Solo lo emite y lo verifica este mismo banco.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

pub const EMISOR: &str = "banco-valmed";
pub const AUDIENCIA: &str = "api-market";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reclamos {
    /// Id de la clave de API.
    pub sub: Uuid,
    /// Titular de la cuenta.
    pub name: String,
    /// Número de cuenta que recibe los cobros.
    pub company_id: String,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
}

impl Reclamos {
    pub fn new(clave_id: Uuid, nombre: &str, cuenta: &str, ahora: i64, vigencia_s: i64) -> Self {
        Self {
            sub: clave_id,
            name: nombre.to_owned(),
            company_id: cuenta.to_owned(),
            iss: EMISOR.to_owned(),
            aud: AUDIENCIA.to_owned(),
            iat: ahora,
            exp: ahora + vigencia_s,
        }
    }
}

fn mac(secreto: &[u8], datos: &str) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secreto).expect("HMAC acepta claves de cualquier largo");
    mac.update(datos.as_bytes());
    mac
}

pub fn emitir(secreto: &[u8], reclamos: &Reclamos) -> String {
    let cabecera = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let cuerpo =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(reclamos).expect("los reclamos se serializan"));
    let firmado = format!("{cabecera}.{cuerpo}");
    let firma = URL_SAFE_NO_PAD.encode(mac(secreto, &firmado).finalize().into_bytes());
    format!("{firmado}.{firma}")
}

/// Reclamos del token si la firma es válida, es de este banco y no venció.
pub fn verificar(secreto: &[u8], token: &str, ahora: i64) -> Option<Reclamos> {
    let (firmado, firma) = token.rsplit_once('.')?;
    let (cabecera, cuerpo) = firmado.split_once('.')?;
    let firma = URL_SAFE_NO_PAD.decode(firma).ok()?;
    mac(secreto, firmado).verify_slice(&firma).ok()?;

    let cabecera: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cabecera).ok()?).ok()?;
    if cabecera.get("alg")?.as_str()? != "HS256" {
        return None;
    }
    let reclamos: Reclamos = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cuerpo).ok()?).ok()?;
    (reclamos.exp > ahora && reclamos.iss == EMISOR && reclamos.aud == AUDIENCIA)
        .then_some(reclamos)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRETO: &[u8] = b"secreto-de-prueba-de-32-bytes!!!";

    fn reclamos(ahora: i64) -> Reclamos {
        Reclamos::new(Uuid::nil(), "Comercio Demo", "1001-000005", ahora, 3600)
    }

    #[test]
    fn verifica_un_token_propio() {
        let token = emitir(SECRETO, &reclamos(1_000));
        assert_eq!(token.split('.').count(), 3);
        assert_eq!(verificar(SECRETO, &token, 1_001), Some(reclamos(1_000)));
    }

    #[test]
    fn rechaza_tokens_vencidos_alterados_o_de_otro_secreto() {
        let token = emitir(SECRETO, &reclamos(1_000));
        assert!(verificar(SECRETO, &token, 4_600).is_none());
        assert!(verificar(b"otro-secreto", &token, 1_001).is_none());
        let (cabecera, resto) = token.split_once('.').unwrap();
        let (_, firma) = resto.split_once('.').unwrap();
        let falso = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Reclamos {
                exp: 99_999,
                ..reclamos(1_000)
            })
            .unwrap(),
        );
        assert!(verificar(SECRETO, &format!("{cabecera}.{falso}.{firma}"), 1_001).is_none());
        assert!(verificar(SECRETO, "no-es-un-jwt", 1_001).is_none());
    }
}
