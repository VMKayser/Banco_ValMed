//! Endpoints públicos de un cobro: estado, imagen del QR y eventos en vivo (SSE).
//! No piden credenciales: el id es un UUID v4 imposible de adivinar, y no muestran
//! datos del pagador. Tienen CORS abierto para que cualquier front los use.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::header;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use futures_util::stream::Stream;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use super::extractores::{Ruta, UrlBase};
use crate::error::{CuerpoError, ErrorBanco};
use crate::infra::qr;
use crate::servicio::cobros::payload;
use crate::servicio::vistas::EstadoPublico;
use crate::servicio::Banco;

/// Estado de un cobro (para que el front del comercio sepa si ya le pagaron).
#[utoipa::path(
    get,
    path = "/api/v1/public/qr/{id}",
    tag = "público",
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses(
        (status = 200, body = EstadoPublico),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
    )
)]
pub async fn estado(
    State(banco): State<Banco>,
    Ruta(id): Ruta<Uuid>,
) -> Result<Json<EstadoPublico>, ErrorBanco> {
    Ok(Json(banco.estado_publico(id).await?))
}

/// QR en PNG (320 px o más), listo para `<img src>`.
#[utoipa::path(
    get,
    path = "/api/v1/public/qr/{id}/image.png",
    tag = "público",
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses(
        (status = 200, content_type = "image/png", description = "Imagen PNG del QR"),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
    )
)]
pub async fn imagen_png(
    State(banco): State<Banco>,
    Ruta(id): Ruta<Uuid>,
    UrlBase(base): UrlBase,
) -> Result<impl IntoResponse, ErrorBanco> {
    banco.estado_publico(id).await?;
    let png = qr::png(&payload(&base, id))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        png,
    ))
}

/// QR en SVG.
#[utoipa::path(
    get,
    path = "/api/v1/public/qr/{id}/image.svg",
    tag = "público",
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses(
        (status = 200, content_type = "image/svg+xml", description = "Imagen SVG del QR"),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
    )
)]
pub async fn imagen_svg(
    State(banco): State<Banco>,
    Ruta(id): Ruta<Uuid>,
    UrlBase(base): UrlBase,
) -> Result<impl IntoResponse, ErrorBanco> {
    banco.estado_publico(id).await?;
    let svg = qr::svg(&payload(&base, id))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        svg,
    ))
}

/// Server-Sent Events: manda el estado actual y luego cada cambio (evento
/// `status`, mismo JSON que el estado público). Cierra cuando el cobro queda
/// PAID, EXPIRED, CANCELLED o REFUNDED. En el navegador:
/// `new EventSource(url).addEventListener('status', e => …)`.
#[utoipa::path(
    get,
    path = "/api/v1/public/qr/{id}/events",
    tag = "público",
    params(("id" = Uuid, Path, description = "Id del cobro")),
    responses(
        (status = 200, content_type = "text/event-stream", description = "Eventos `status` con un EstadoPublico en `data`"),
        (status = 404, description = "QR_NOT_FOUND", body = CuerpoError),
    )
)]
pub async fn eventos(
    State(banco): State<Banco>,
    Ruta(id): Ruta<Uuid>,
) -> Result<impl IntoResponse, ErrorBanco> {
    // Primero suscribirse y después leer: así no se pierde un pago que ocurra en medio.
    let receptor = banco.suscribir();
    let inicial = banco.estado_publico(id).await?;
    let flujo = flujo_de_estados(banco, id, inicial, receptor);
    Ok((
        [("x-accel-buffering", "no")],
        Sse::new(flujo).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))),
    ))
}

fn flujo_de_estados(
    banco: Banco,
    id: Uuid,
    inicial: EstadoPublico,
    mut receptor: tokio::sync::broadcast::Receiver<crate::servicio::EventoQr>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        let mut ultimo = inicial.status;
        let mut vence = inicial.expires_at;
        yield Ok(evento(&inicial));
        if ultimo.es_final() {
            return;
        }
        loop {
            // Además de los pagos, hay que avisar cuando vence aunque nadie haga nada.
            let espera = (vence - Utc::now()).to_std().unwrap_or_default() + Duration::from_millis(500);
            let revisar = tokio::select! {
                recibido = receptor.recv() => match recibido {
                    Ok(e) => e.id == id,
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => break,
                },
                _ = tokio::time::sleep(espera) => true,
            };
            if !revisar {
                continue;
            }
            match banco.estado_publico(id).await {
                Ok(estado) => {
                    vence = estado.expires_at;
                    if estado.status != ultimo {
                        ultimo = estado.status;
                        yield Ok(evento(&estado));
                    }
                    if ultimo.es_final() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(qr = %id, error = %e, "no se pudo leer el cobro para SSE");
                    break;
                }
            }
        }
    }
}

fn evento(estado: &EstadoPublico) -> Event {
    Event::default()
        .event("status")
        .json_data(estado)
        .unwrap_or_else(|_| Event::default().event("status"))
}
