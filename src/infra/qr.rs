//! Imagen del QR. Lo que va adentro es una URL a la app del banco
//! (`<base>/?p=<id>`): la cámara normal del celular la abre directo.

use anyhow::{anyhow, Result};
use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};

const LADO_MINIMO: u32 = 320;

fn codificar(texto: &str) -> Result<QrCode> {
    QrCode::with_error_correction_level(texto.as_bytes(), EcLevel::M)
        .map_err(|e| anyhow!("no se pudo generar el QR: {e:?}"))
}

pub fn svg(texto: &str) -> Result<String> {
    Ok(codificar(texto)?
        .render::<svg::Color>()
        .min_dimensions(LADO_MINIMO, LADO_MINIMO)
        .dark_color(svg::Color("#0b3b32"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .build())
}

pub fn png(texto: &str) -> Result<Vec<u8>> {
    let imagen = codificar(texto)?
        .render::<image::Luma<u8>>()
        .min_dimensions(LADO_MINIMO, LADO_MINIMO)
        .quiet_zone(true)
        .build();
    let mut bytes = Vec::new();
    image::DynamicImage::ImageLuma8(imagen).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genera_png_y_svg() {
        let url = "http://localhost:8086/?p=0b8e4c3a-5d6f-4a1b-9c2d-7e8f9a0b1c2d";
        let png = png(url).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(svg(url).unwrap().contains("<svg"));
    }
}
