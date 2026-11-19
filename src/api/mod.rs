//! Rutas HTTP. Swagger UI en `/docs`, contrato en `/docs/openapi.json`, y la web
//! app (archivos estáticos) en `/`.

mod app;
mod extractores;
mod integracion;
mod market;
mod publico;

use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{any, get, patch, post};
use axum::{Json, Router};
use serde::Serialize;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing::Instrument;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

pub use extractores::base_desde_cabeceras;

use crate::error::{ErrorBanco, CORRELACION};
use crate::servicio::Banco;
use extractores::UrlBase;

pub const CABECERA_CORRELACION: &str = "x-correlation-id";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Banco ValMed (simulado)",
        version = "1.0.0",
        description = "Banco simulado, sin dinero real, para cobrar con QR dinámico.\n\n\
            * **api-market**: la misma API Market (Pago QR) del BNB, para poder migrar al banco real cambiando solo la URL y las credenciales.\n\
            * **integración**: API propia de ValMed con extras (eventos en vivo, devoluciones, `X-Api-Key`).\n\
            * **público**: estado, imagen y eventos en vivo (SSE) de un cobro, sin credenciales.\n\
            * **sandbox**: pagar un cobro por API, sin la app.\n\
            * **app**: lo que usa la web app del banco (sesión con `Bearer`).\n\n\
            Montos como string con 2 decimales (`\"350.00\"`), moneda BOB, fechas ISO-8601 UTC. \
            Errores con el cuerpo `{ code, message, details, traceId }`."
    ),
    paths(
        market::token,
        market::cambiar_credenciales,
        market::generar_qr,
        market::qrs_por_fecha,
        market::estado_qr,
        market::anular_qr,
        integracion::crear_cobro,
        integracion::listar_cobros,
        integracion::cobro,
        integracion::anular,
        integracion::devolver,
        integracion::listar_avisos,
        integracion::reenviar_aviso,
        integracion::pagar_sandbox,
        publico::estado,
        publico::imagen_png,
        publico::imagen_svg,
        publico::eventos,
        app::abrir_cuenta,
        app::iniciar_sesion,
        app::cerrar_sesion,
        app::perfil,
        app::movimientos,
        app::depositar,
        app::crear_cobro,
        app::listar_cobros,
        app::cobro,
        app::cobro_por_codigo,
        app::pagar,
        app::anular,
        app::devolver,
        app::listar_claves,
        app::crear_clave,
        app::cambiar_webhook,
        app::revocar_clave,
        app::listar_avisos,
        app::reenviar_aviso,
        configuracion,
        salud,
    ),
    components(schemas(
        crate::servicio::vistas::EventoAviso,
        crate::servicio::vistas::NotificacionMarket
    )),
    modifiers(&Seguridad),
    tags(
        (name = "api-market", description = "Compatible con la API Market (Pago QR) del BNB. Token en `Authorization: Bearer`. Mira siempre `success`: un rechazo responde HTTP 200 con `success: false`. Al pagarse un QR, el banco hace POST a tu URL de notificación con el esquema `NotificacionMarket`."),
        (name = "integración", description = "Para el backend de otro sistema: crear y gestionar cobros QR (X-Api-Key)"),
        (name = "público", description = "Estado, imagen y eventos en vivo de un cobro, sin credenciales"),
        (name = "sandbox", description = "Pagar por API, sin la app (solo con BANK_SANDBOX=true)"),
        (name = "app", description = "Web app del banco: cuentas, pagos, cobros y claves (sesión Bearer)"),
        (name = "operación", description = "Configuración y estado del servicio"),
    )
)]
pub struct ApiDoc;

struct Seguridad;

impl Modify for Seguridad {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let componentes = openapi.components.get_or_insert_with(Default::default);
        componentes.add_security_scheme(
            "apiKey",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-Api-Key",
                "Clave creada en la app, sección Desarrolladores",
            ))),
        );
        componentes.add_security_scheme(
            "tokenMarket",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some(
                        "Token de POST /ClientAuthentication.API/api/v1/auth/token (dura 1 h)",
                    ))
                    .build(),
            ),
        );
        componentes.add_security_scheme(
            "sesion",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("Token de POST /api/v1/app/sessions"))
                    .build(),
            ),
        );
    }
}

/// El contrato en YAML (`banco-valmed openapi` lo imprime).
pub fn contrato_yaml() -> anyhow::Result<String> {
    ApiDoc::openapi()
        .to_yaml()
        .map_err(|e| anyhow::anyhow!("no se pudo generar el YAML: {e}"))
}

pub fn router(banco: Banco) -> Router {
    let publico = Router::new()
        .route("/api/v1/public/qr/{id}", get(publico::estado))
        .route("/api/v1/public/qr/{id}/image.png", get(publico::imagen_png))
        .route("/api/v1/public/qr/{id}/image.svg", get(publico::imagen_svg))
        .route("/api/v1/public/qr/{id}/events", get(publico::eventos))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET]),
        );

    let estaticos = ServeDir::new(&banco.config().carpeta_estatica);

    Router::new()
        .merge(publico)
        // API Market (compatible con el BNB)
        .route(
            "/ClientAuthentication.API/api/v1/auth/token",
            post(market::token),
        )
        .route(
            "/ClientAuthentication.API/api/v1/auth/UpdateCredentials",
            post(market::cambiar_credenciales),
        )
        .route(
            "/QRSimple.API/api/v1/main/getQRWithImageAsync",
            post(market::generar_qr),
        )
        .route(
            "/QRSimple.API/api/v1/main/getQRbyGenerationDateAsync",
            post(market::qrs_por_fecha),
        )
        .route(
            "/QRSimple.API/api/v1/main/getQRStatusAsync",
            post(market::estado_qr),
        )
        .route(
            "/QRSimple.API/api/v1/main/CancelQRByIdAsync",
            post(market::anular_qr),
        )
        .route(
            "/ClientAuthentication.API/{*resto}",
            any(market::no_encontrado),
        )
        .route("/QRSimple.API/{*resto}", any(market::no_encontrado))
        // Integración
        .route(
            "/api/v1/qr",
            post(integracion::crear_cobro).get(integracion::listar_cobros),
        )
        .route("/api/v1/qr/{id}", get(integracion::cobro))
        .route("/api/v1/qr/{id}/cancel", post(integracion::anular))
        .route(
            "/api/v1/payments/{referencia}/refunds",
            post(integracion::devolver),
        )
        .route(
            "/api/v1/webhook-deliveries",
            get(integracion::listar_avisos),
        )
        .route(
            "/api/v1/webhook-deliveries/{id}/redeliver",
            post(integracion::reenviar_aviso),
        )
        .route(
            "/api/v1/sandbox/qr/{id}/pay",
            post(integracion::pagar_sandbox),
        )
        // App
        .route("/api/v1/app/customers", post(app::abrir_cuenta))
        .route("/api/v1/app/sessions", post(app::iniciar_sesion))
        .route(
            "/api/v1/app/sessions/current",
            axum::routing::delete(app::cerrar_sesion),
        )
        .route("/api/v1/app/me", get(app::perfil))
        .route("/api/v1/app/movements", get(app::movimientos))
        .route("/api/v1/app/deposits", post(app::depositar))
        .route(
            "/api/v1/app/qr",
            post(app::crear_cobro).get(app::listar_cobros),
        )
        .route("/api/v1/app/qr/{id}", get(app::cobro))
        .route("/api/v1/app/qr/{id}/payments", post(app::pagar))
        .route("/api/v1/app/qr/{id}/cancel", post(app::anular))
        .route("/api/v1/app/qr-codes/{codigo}", get(app::cobro_por_codigo))
        .route(
            "/api/v1/app/payments/{referencia}/refunds",
            post(app::devolver),
        )
        .route(
            "/api/v1/app/api-keys",
            get(app::listar_claves).post(app::crear_clave),
        )
        .route(
            "/api/v1/app/api-keys/{id}",
            patch(app::cambiar_webhook).delete(app::revocar_clave),
        )
        .route("/api/v1/app/webhook-deliveries", get(app::listar_avisos))
        .route(
            "/api/v1/app/webhook-deliveries/{id}/redeliver",
            post(app::reenviar_aviso),
        )
        // Operación
        .route("/api/v1/config", get(configuracion))
        .route("/health", get(salud))
        .route("/api/{*resto}", any(no_encontrado))
        .with_state(banco)
        .merge(SwaggerUi::new("/docs").url("/docs/openapi.json", ApiDoc::openapi()))
        .fallback_service(estaticos)
        .layer(middleware::from_fn(correlacion))
}

/// Lee (o genera) el X-Correlation-Id, lo pone en los logs y en la respuesta, y
/// registra una línea por petición a la API.
async fn correlacion(peticion: Request, siguiente: Next) -> Response {
    let id = peticion
        .headers()
        .get(CABECERA_CORRELACION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 128 && v.chars().all(|c| c.is_ascii_graphic()))
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let ruta = peticion.uri().path().to_owned();
    let registrar = ruta.starts_with("/api/") || ruta.contains(".API/");
    let span = tracing::info_span!(
        "http",
        correlation_id = %id,
        method = %peticion.method(),
        path = %ruta,
    );
    let inicio = Instant::now();
    let mut respuesta = CORRELACION
        .scope(id.clone(), siguiente.run(peticion))
        .instrument(span.clone())
        .await;

    if registrar {
        span.in_scope(|| {
            tracing::info!(
                status = respuesta.status().as_u16(),
                duration_ms = inicio.elapsed().as_millis() as u64,
                "petición atendida"
            )
        });
    }
    if let Ok(valor) = HeaderValue::from_str(&id) {
        respuesta.headers_mut().insert(CABECERA_CORRELACION, valor);
    }
    respuesta
}

async fn no_encontrado() -> ErrorBanco {
    ErrorBanco::NoEncontrado
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct Configuracion {
    #[schema(example = "Banco ValMed")]
    bank_name: String,
    /// URL pública que usa el banco para armar los QR.
    #[schema(example = "http://localhost:8086")]
    public_url: String,
    sandbox: bool,
    /// Hay clientes demo (7000001…7000004, PIN 123456).
    demo_accounts: bool,
    #[schema(value_type = String, example = "1000.00")]
    initial_balance: rust_decimal::Decimal,
    #[schema(example = "BOB")]
    currency: String,
}

/// Datos que usa la web app al arrancar.
#[utoipa::path(
    get,
    path = "/api/v1/config",
    tag = "operación",
    responses((status = 200, body = Configuracion))
)]
async fn configuracion(State(banco): State<Banco>, UrlBase(base): UrlBase) -> Json<Configuracion> {
    let config = banco.config();
    Json(Configuracion {
        bank_name: "Banco ValMed".to_owned(),
        public_url: base,
        sandbox: config.sandbox,
        demo_accounts: config.semilla_demo,
        initial_balance: config.saldo_inicial,
        currency: crate::dominio::dinero::MONEDA.to_owned(),
    })
}

#[derive(Serialize, ToSchema)]
struct Salud {
    #[schema(example = "UP")]
    status: String,
    #[schema(example = "UP")]
    database: String,
}

/// Estado del servicio y de su base (lo usa el healthcheck de Docker).
#[utoipa::path(
    get,
    path = "/health",
    tag = "operación",
    responses(
        (status = 200, body = Salud),
        (status = 503, body = Salud),
    )
)]
async fn salud(State(banco): State<Banco>) -> (StatusCode, Json<Salud>) {
    let base_ok = sqlx::query("SELECT 1").execute(banco.pool()).await.is_ok();
    let (estado, texto) = if base_ok {
        (StatusCode::OK, "UP")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "DOWN")
    };
    (
        estado,
        Json(Salud {
            status: texto.to_owned(),
            database: texto.to_owned(),
        }),
    )
}

#[cfg(test)]
mod tests {
    /// Si falla: `docker compose run --rm banco openapi > openapi/banco-valmed.v1.yaml`
    /// (o `cargo run -- openapi > …`) y revisar el diff.
    #[test]
    fn el_contrato_publicado_esta_al_dia() {
        let generado = super::contrato_yaml().unwrap();
        let publicado = include_str!("../../openapi/banco-valmed.v1.yaml");
        assert!(
            generado == publicado,
            "openapi/banco-valmed.v1.yaml no coincide con el código; regenéralo con `banco-valmed openapi`"
        );
    }
}
