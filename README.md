# Banco ValMed

Banco **simulado** (sin dinero real) para cobrar y pagar con **QR dinámicos**. Se pueden crear cobros desde su web app o desde otro sistema, y el cobro **se valida solo** cuando alguien lo paga (eventos en vivo y notificación al sistema que cobra).

- **Para integrarlo con la API Market (igual a la del BNB, migrable al banco real):** [docs/API_MARKET_PAGO_QR.md](docs/API_MARKET_PAGO_QR.md)
- **Para usar la API propia (eventos en vivo, devoluciones):** [docs/GUIA_INTEGRACION.md](docs/GUIA_INTEGRACION.md)
- **Diseño y decisiones:** [docs/DISENO.md](docs/DISENO.md)
- **Contrato:** [openapi/banco-valmed.v1.yaml](openapi/banco-valmed.v1.yaml) · Swagger en `/docs`

## Levantarlo

```bash
cp .env.example .env            # cambia BANK_DB_PASSWORD
docker compose up -d --build
```

| | |
|---|---|
| Web app | http://localhost:8086 |
| Swagger | http://localhost:8086/docs |
| Desde otro contenedor | `http://banco:8080` (red Docker `banco-valmed`) |

Cuentas de prueba, todas con PIN `123456`: **7000001** Rosa (Bs 2 000), **7000002** Juan (Bs 50), **7000003** Luis y **7000004** Carla (Bs 1 000 cada uno).

## Probar con el celular

La cámara del navegador exige HTTPS, así que hay que exponer el banco con un túnel:

```bash
cloudflared tunnel --url http://127.0.0.1:8086
```

1. Abre la URL del túnel en el celular y entra con una cuenta de prueba.
2. En la laptop, entra con otra cuenta y elige **Cobrar** para generar un QR.
3. En el celular, elige **Pagar QR**, escanéalo y confirma con el PIN. La pantalla de la laptop cambia a **Pagado** sola.

Si los QR los crea otro sistema por la red interna, pon la URL del túnel en `BANK_PUBLIC_URL` (en `.env`) y ejecuta `docker compose up -d`.

## Qué tiene

| Pantalla | Qué hace |
|---|---|
| Entrar / Abrir cuenta | C.I. (+ complemento) y PIN de 6 dígitos; cada cuenta nueva nace con Bs 1 000 |
| Inicio | Saldo, número de cuenta, accesos y últimos movimientos |
| Pagar QR | Cámara (`BarcodeDetector` o `jsQR`), código de 8 caracteres o foto del QR → confirmar con PIN → comprobante |
| Cobrar | Monto, detalle y vigencia → QR, código y cuenta regresiva; pasa a «Pagado por …» en vivo; anular o devolver |
| Depositar | Saldo simulado (hasta Bs 5 000 por vez) |
| Movimientos | Historial paginado con el saldo después de cada uno |
| Desarrolladores | Claves (`accountId` y `authorizationId` de la API Market, o `X-Api-Key`), URL de notificación y avisos enviados con botón «Reenviar» |

## Desarrollo

No hace falta Rust instalado: todo corre en Docker.

```bash
# Imagen con clippy y rustfmt (una vez)
printf 'FROM rust:1.90-slim\nRUN rustup component add clippy rustfmt\n' | docker build -t banco-valmed-dev -
R="docker run --rm -v $PWD:/app -v banco-valmed-registry:/usr/local/cargo/registry -v banco-valmed-target:/app/target -w /app banco-valmed-dev"

$R cargo test                       # 40 pruebas unitarias + contrato OpenAPI
$R cargo clippy --all-targets
$R cargo fmt
$R sh -c 'cargo run -q -- openapi > openapi/banco-valmed.v1.yaml'   # regenerar el contrato
```

Docker deja como root los archivos que crea. Para devolvértelos:

```bash
$R sh -c 'find /app -path /app/target -prune -o -user 0 -exec chown 1000:1000 {} +'
```

La prueba `el_contrato_publicado_esta_al_dia` falla si cambias la API y no regeneras `openapi/banco-valmed.v1.yaml`.

## Variables de entorno

Ver [.env.example](.env.example).

| Variable | Por defecto | Qué hace |
|---|---|---|
| `BANK_DB_PASSWORD` | — | Contraseña de `banco-db` (obligatoria) |
| `BANK_HOST_PORT` | `8086` | Puerto publicado en el host |
| `BANK_PUBLIC_URL` | vacía | URL fija para los QR. Si está vacía, se toma de cada petición |
| `BANK_SANDBOX` | `true` | Habilita pagar por API sin la app |
| `BANK_INITIAL_BALANCE` | `1000.00` | Saldo con el que nace cada cuenta |
| `BANK_SEED_DEMO` | `true` | Crea las cuentas de prueba |
| `BANK_SEED_API_KEY`, `BANK_SEED_WEBHOOK_SECRET`, `BANK_SEED_WEBHOOK_URL` | vacías | Clave fija para compartir con otro equipo (también es el `authorizationId`; el `accountId` sale en el log al arrancar) |
| `BANK_TOKEN_SECRET` | vacía | Firma los tokens de la API Market (mínimo 32 caracteres). Vacía: se genera al arrancar y los tokens dejan de servir al reiniciar |
