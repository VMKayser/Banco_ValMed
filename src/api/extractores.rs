//! Extractores: cuerpo/consulta/ruta con errores en el formato común, la sesión de
//! la app, la clave de API del integrador y la URL pública del banco.

use std::convert::Infallible;

use axum::extract::{FromRequest, FromRequestParts};
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::request::Parts;
use axum::http::HeaderMap;

use crate::error::ErrorBanco;
use crate::servicio::claves::Integrador;
use crate::servicio::clientes::ClienteActual;
use crate::servicio::Banco;

/// JSON del cuerpo; si no se puede leer responde 400 `VALIDATION_ERROR`.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ErrorBanco))]
pub struct Cuerpo<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ErrorBanco))]
pub struct Consulta<T>(pub T);

/// Un id mal formado se responde como 404.
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ErrorBanco))]
pub struct Ruta<T>(pub T);

/// Cliente con sesión en la app (`Authorization: Bearer <token>`).
pub struct Sesion(pub ClienteActual);

impl FromRequestParts<Banco> for Sesion {
    type Rejection = ErrorBanco;

    async fn from_request_parts(parts: &mut Parts, banco: &Banco) -> Result<Self, ErrorBanco> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(ErrorBanco::NoAutenticado("falta el token de sesión"))?;
        banco
            .sesion(token)
            .await?
            .map(Sesion)
            .ok_or(ErrorBanco::NoAutenticado("la sesión venció o no existe"))
    }
}

/// Integrador con clave de API (`X-Api-Key`).
pub struct ConClave(pub Integrador);

impl FromRequestParts<Banco> for ConClave {
    type Rejection = ErrorBanco;

    async fn from_request_parts(parts: &mut Parts, banco: &Banco) -> Result<Self, ErrorBanco> {
        let clave = parts
            .headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(ErrorBanco::NoAutenticado("falta la cabecera X-Api-Key"))?;
        banco
            .autenticar_clave(clave)
            .await?
            .map(ConClave)
            .ok_or(ErrorBanco::NoAutenticado(
                "clave de API inválida o revocada",
            ))
    }
}

/// `X-Api-Key` o, si no viene, el token Bearer de la API Market (lo usa el
/// sandbox, para que quien integra solo con la API Market pueda probar pagos).
pub struct ClaveOToken(pub Integrador);

impl FromRequestParts<Banco> for ClaveOToken {
    type Rejection = ErrorBanco;

    async fn from_request_parts(parts: &mut Parts, banco: &Banco) -> Result<Self, ErrorBanco> {
        if parts.headers.contains_key("x-api-key") {
            return ConClave::from_request_parts(parts, banco)
                .await
                .map(|ConClave(i)| Self(i));
        }
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(ErrorBanco::NoAutenticado(
                "falta X-Api-Key o el token de la API Market",
            ))?;
        banco
            .integrador_market(token)
            .await?
            .map(Self)
            .ok_or(ErrorBanco::NoAutenticado(
                "token de la API Market inválido o vencido",
            ))
    }
}

/// URL pública del banco, sin barra final: `BANK_PUBLIC_URL` si está configurada;
/// si no, la que usó el cliente (`X-Forwarded-Proto/Host/Prefix` o `Host`).
pub struct UrlBase(pub String);

impl FromRequestParts<Banco> for UrlBase {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, banco: &Banco) -> Result<Self, Infallible> {
        if let Some(url) = &banco.config().url_publica {
            return Ok(Self(url.clone()));
        }
        let config = banco.config();
        Ok(Self(base_desde_cabeceras(
            &parts.headers,
            config.puerto_host.unwrap_or(config.puerto_http),
        )))
    }
}

/// `puerto` es el que se usa con `localhost` cuando el host no sirve afuera.
pub fn base_desde_cabeceras(cabeceras: &HeaderMap, puerto: u16) -> String {
    let leer = |nombre: &str| {
        cabeceras
            .get(nombre)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let esquema = match leer("x-forwarded-proto").as_deref() {
        Some("https") => "https",
        _ => "http",
    };
    let host = leer("x-forwarded-host")
        .or_else(|| leer(HOST.as_str()))
        .filter(|h| {
            h.len() <= 255
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        })
        .filter(|h| sirve_afuera(h))
        .unwrap_or_else(|| format!("localhost:{puerto}"));
    let prefijo = leer("x-forwarded-prefix")
        .filter(|p| {
            p.starts_with('/')
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_'))
        })
        .map(|p| p.trim_end_matches('/').to_owned())
        .unwrap_or_default();
    format!("{esquema}://{host}{prefijo}")
}

/// Un nombre de servicio de Docker (`banco`, `banco:8080`) no lo resuelve un
/// navegador ni un celular; `localhost`, un dominio o una IP sí.
fn sirve_afuera(host: &str) -> bool {
    if host.starts_with('[') {
        return true; // IPv6 literal
    }
    let nombre = host.rsplit_once(':').map_or(host, |(n, _)| n);
    nombre.eq_ignore_ascii_case("localhost") || nombre.contains('.')
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn cabeceras(pares: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pares {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn arma_la_base_con_el_host() {
        let h = cabeceras(&[("host", "localhost:8086")]);
        assert_eq!(base_desde_cabeceras(&h, 8080), "http://localhost:8086");
    }

    #[test]
    fn respeta_un_proxy_con_https_y_prefijo() {
        let h = cabeceras(&[
            ("host", "nginx"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "abc.trycloudflare.com"),
            ("x-forwarded-prefix", "/banco/"),
        ]);
        assert_eq!(
            base_desde_cabeceras(&h, 8080),
            "https://abc.trycloudflare.com/banco"
        );
    }

    #[test]
    fn un_nombre_interno_de_docker_se_cambia_por_localhost() {
        let h = cabeceras(&[("host", "banco:8080")]);
        assert_eq!(base_desde_cabeceras(&h, 8086), "http://localhost:8086");
        let h = cabeceras(&[("host", "192.168.1.20:8086")]);
        assert_eq!(base_desde_cabeceras(&h, 8086), "http://192.168.1.20:8086");
    }

    #[test]
    fn ignora_hosts_raros() {
        let h = cabeceras(&[("host", "evil.com/<script>")]);
        assert_eq!(base_desde_cabeceras(&h, 8080), "http://localhost:8080");
    }
}
