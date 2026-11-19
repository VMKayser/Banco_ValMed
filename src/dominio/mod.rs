//! Reglas puras, sin base de datos ni HTTP (se prueban con tests unitarios).

pub mod ci;
pub mod codigo;
pub mod dinero;
pub mod firma;
pub mod market;
pub mod pin;

pub use ci::Ci;
