//! Banco ValMed: banco simulado (sin dinero real) con cobros por QR dinámico,
//! cuentas con PIN, claves de API para integradores y avisos firmados.

pub mod api;
pub mod config;
pub mod dominio;
pub mod error;
pub mod infra;
pub mod servicio;

pub use servicio::Banco;
