# Banco ValMed · Pago QR: documentación de servicios (API Market)

Banco ValMed es un banco **simulado**: no mueve dinero real. Esta API copia la **API Market (Pago QR) del Banco Nacional de Bolivia (BNB)**. Tiene las mismas rutas, los mismos campos, las mismas respuestas `{ success, message }` y la misma notificación de pago.

Si tu sistema se integra **solo** con lo que describe este documento, pasar al BNB real consiste en cambiar la URL base y las credenciales (ver [§14](#14-migrar-al-bnb)).

> Fuente: el documento del BNB «Pago QR – Documentación de Servicios – Api Market» y la página de sandbox de su [portal de APIs](https://www.bnb.com.bo/PortalBNB/Api/Sandbox). Los puntos donde el BNB no dice nada, o donde ValMed se aparta, están en [§13](#13-diferencias-con-el-bnb).

## Índice

1. [Introducción](#01-introducción)
2. [Visión general](#02-visión-general)
3. [Autenticación](#03-autenticación)
4. [Diseño](#04-diseño)
5. [Generar token](#05-generar-token)
6. [Actualizar credenciales](#06-actualizar-credenciales)
7. [Generar QR](#07-generar-qr)
8. [Obtener lista de QR generados](#08-obtener-lista-de-qr-generados)
9. [Obtener estado de un QR](#09-obtener-estado-de-un-qr)
10. [Cancelar QR](#10-cancelar-qr)
11. [Notificación de pagos QR](#11-notificación-de-pagos-qr-receivenotification)
12. [Qué datos entrega el QR](#12-qué-datos-entrega-el-qr)
13. [Diferencias con el BNB](#13-diferencias-con-el-bnb)
14. [Migrar al BNB](#14-migrar-al-bnb)
15. [Pruebas sin celular (solo ValMed)](#15-pruebas-sin-celular-solo-valmed)

---

## 01. Introducción

Esta API ofrece servicios para **generar QR de cobro**, **consultar su estado**, **listar los QR de un día** y **anularlos**. Cuando alguien paga un QR, el banco **notifica en línea** a la empresa que lo generó.

Quien paga lo hace desde la web app de Banco ValMed: escanea el QR con el celular y confirma con su PIN.

## 02. Visión general

| Servicio | Método y ruta |
|---|---|
| Generar token | `POST /ClientAuthentication.API/api/v1/auth/token` |
| Actualizar credenciales | `POST /ClientAuthentication.API/api/v1/auth/UpdateCredentials` |
| Generar QR | `POST /QRSimple.API/api/v1/main/getQRWithImageAsync` |
| Obtener lista de QR generados | `POST /QRSimple.API/api/v1/main/getQRbyGenerationDateAsync` |
| Obtener estado de un QR | `POST /QRSimple.API/api/v1/main/getQRStatusAsync` |
| Cancelar QR | `POST /QRSimple.API/api/v1/main/CancelQRByIdAsync` |
| Notificación de pago (la expone **tu** sistema) | `POST <tu URL>` (ej. `…/ReceiveNotification`) |

**URL base**

| Dónde | Banco ValMed | BNB |
|---|---|---|
| Desde tu máquina | `http://localhost:8086` | — |
| Desde un contenedor en la red `banco-valmed` | `http://banco:8080` | — |
| Ambiente de pruebas | — | `http://test.bnb.com.bo` |
| Producción | — | La entrega el BNB (integraciones públicas usan `https://marketapi.bnb.com.bo`) |

Usa las rutas **tal como aparecen aquí, con sus mayúsculas**: ValMed las distingue. Escritas así funcionan en los dos bancos.

Contrato OpenAPI: Swagger en `http://localhost:8086/docs`, grupo **api-market**.

## 03. Autenticación

1. Pide un **token** con tu `accountId` y tu `authorizationId` ([§05](#05-generar-token)).
2. Envía el token en la cabecera de **todas** las llamadas a `QRSimple.API`:

   ```http
   Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
   Content-Type: application/json
   ```

3. El token vence a la hora. Cuando vence, o si el banco se reinicia sin `BANK_TOKEN_SECRET`, los servicios responden **HTTP 401**. Pide otro token y repite la llamada una vez.

**De dónde salen las credenciales**

| | Banco ValMed | BNB |
|---|---|---|
| `accountId` | Web app → **Desarrolladores** → «Crear clave» (se muestra siempre en la lista) | Lo entrega tu ejecutivo del BNB tras firmar el convenio |
| `authorizationId` | Al crear la clave es la misma **clave de API** (`vm_test_…`). Se muestra una sola vez. | Lo entrega el BNB. Hay que cambiarlo con `UpdateCredentials` antes del primer uso. |
| URL de notificación | La pones al crear la clave («URL de notificación») | Se la das al BNB |

> Con la clave fija de la semilla (`BANK_SEED_API_KEY` en el `.env` del banco), el `authorizationId` es esa clave y el `accountId` sale en el log al arrancar: `docker compose logs banco | grep "clave de API de demostración"`.

## 04. Diseño

```mermaid
sequenceDiagram
  autonumber
  participant S as Tu backend
  participant F as Tu front
  participant B as Banco (ValMed o BNB)
  participant A as App del banco (celular)

  S->>B: POST auth/token {accountId, authorizationId}
  B-->>S: {success: true, message: token}
  S->>B: POST getQRWithImageAsync {gloss, amount, expirationDate, ...}
  B-->>S: {id, qr (PNG base64), success: true}
  S->>S: guarda id junto a tu orden
  S-->>F: imagen del QR
  F->>S: ¿ya pagaron? (cada 3 a 5 s)
  A->>B: escanea el QR y paga
  B->>S: POST ReceiveNotification {QRId, VoucherId, ...}
  S->>B: POST getQRStatusAsync {qrId}
  B-->>S: {statusId: 2, voucherId}
  S-->>B: {success: true, message: OK}
  S->>S: marca la orden como pagada
  F->>S: ¿ya pagaron?
  S-->>F: pagado
```

**Reglas de respuesta**

- Mira **siempre** `success`. Cuando el banco rechaza una operación (QR inexistente, monto inválido, credenciales malas…) responde **HTTP 200 con `success: false`** y el motivo en `message`, igual que el BNB.
- **HTTP 400** si el cuerpo no es un JSON con los campos obligatorios, o falta `Content-Type: application/json`.
- **HTTP 401** si falta el token, es inválido o venció.
- **HTTP 500 o 503** si el banco tiene un problema: reintenta más tarde.
- Los nombres de campo no distinguen mayúsculas (`qrId`, `QrId` y `QRId` valen). Los números se aceptan también como texto (`"amount": "20"`) y los booleanos como `"true"`/`"false"`.

---

## 05. Generar token

`POST /ClientAuthentication.API/api/v1/auth/token`

Genera el token de seguridad para todas las solicitudes posteriores. Recibe el `accountId` y el `authorizationId`.

**Headers**

| Key | Value |
|---|---|
| Content-Type | application/json |

**Body**

| Key | Value | Description |
|---|---|---|
| accountId | `s9CG8FE7Id75ef2jeX9bUA==` | Identificador de la clave (público). |
| authorizationId | `vm_test_…` | Secreto de la clave. |

**Example request**

```bash
curl -X POST "http://localhost:8086/ClientAuthentication.API/api/v1/auth/token" \
  -H "Content-Type: application/json" \
  -d '{"accountId":"s9CG8FE7Id75ef2jeX9bUA==","authorizationId":"vm_test_..."}'
```

**Example response 200 OK**

```json
{
  "success": true,
  "message": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiI…"
}
```

| Key | Description |
|---|---|
| success | `true` si se generó el token. |
| message | El token. Si `success` es `false`, el motivo (`"Credenciales inválidas"`). |

## 06. Actualizar credenciales

`POST /ClientAuthentication.API/api/v1/auth/UpdateCredentials`

Cambia el `authorizationId`. **En el BNB es obligatorio hacerlo la primera vez, antes de usar los demás servicios.** En ValMed es opcional, y la cabecera `X-Api-Key` de la API propia no cambia.

**Body**

| Key | Value | Description |
|---|---|---|
| AccountId | `s9CG8FE7Id75ef2jeX9bUA==` | Tu `accountId`. |
| actualAuthorizationId | `vm_test_…` | El `authorizationId` actual. |
| newAuthorizationId | `Naciona0123456*` | El nuevo. Debe tener **15 caracteres o más**, **al menos una letra**, **un número** y **un carácter especial** de `+ - / * $ < > ! ¡ # \| =`. |

**Example request**

```bash
curl -X POST "http://localhost:8086/ClientAuthentication.API/api/v1/auth/UpdateCredentials" \
  -H "Content-Type: application/json" \
  -d '{"AccountId":"s9CG8FE7Id75ef2jeX9bUA==","actualAuthorizationId":"vm_test_...","newAuthorizationId":"Naciona0123456*"}'
```

**Example response 200 OK**

```json
{ "success": true, "message": "" }
```

Si la clave nueva no cumple las reglas, o las credenciales actuales no coinciden, responde `success: false` con el motivo.

## 07. Generar QR

`POST /QRSimple.API/api/v1/main/getQRWithImageAsync`

Genera un QR de cobro y devuelve la imagen.

**Headers**: `Authorization: Bearer <token>` y `Content-Type: application/json`.

**Body**

| Key | Value | Description |
|---|---|---|
| currency | `BOB` | Moneda. En ValMed solo `BOB`. |
| gloss | `Entrada A-014` | Descripción del pago, de 1 a 140 caracteres. La ve quien paga y vuelve en la notificación. |
| amount | `350` | Monto, de 0.01 a 100000 con 2 decimales como máximo. |
| singleUse | `true` | QR de un solo pago. En ValMed solo `true`. |
| expirationDate | `2026-11-18` | **Último día** en que se puede pagar (`AAAA-MM-DD`, hora de Bolivia). Va de hoy a 365 días. El QR vence a las 00:00 del día siguiente. |
| additionalData | `orden-6f1c2a9e` | Opcional, hasta 500 caracteres. Úsalo para tu id de orden: vuelve en la notificación. |
| destinationAccountId | `1` | Opcional. `1` = cuenta en bolivianos (la única en ValMed). En el BNB, `2` = cuenta en moneda extranjera. |

**Example request**

```bash
curl -X POST "http://localhost:8086/QRSimple.API/api/v1/main/getQRWithImageAsync" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
        "currency": "BOB",
        "gloss": "Entrada A-014",
        "amount": 350,
        "singleUse": true,
        "expirationDate": "2026-11-18",
        "additionalData": "orden-6f1c2a9e",
        "destinationAccountId": "1"
      }'
```

**Example response 200 OK**

```json
{
  "id": "51",
  "qr": "iVBORw0KGgoAAAANSUhEUgAAAUAAAAFACAAAAADo+/p2AAAH…",
  "success": true,
  "message": null
}
```

| Key | Description |
|---|---|
| id | Identificador del QR, como **texto**. Guárdalo junto a tu orden: llega como `QRId` en la notificación y se usa como `qrId` en las consultas. |
| qr | Imagen PNG del QR en base64. Muéstrala tal cual: `<img src="data:image/png;base64,{qr}">`. |
| success / message | `false` y el motivo si no se generó. |

## 08. Obtener lista de QR generados

`POST /QRSimple.API/api/v1/main/getQRbyGenerationDateAsync`

Devuelve los QR que tu cuenta generó un día determinado (hora de Bolivia), con su estado y el resultado de la notificación. Sirve para **conciliar** al final del día.

**Body**

| Key | Value | Description |
|---|---|---|
| generationDate | `2026-11-18` | Día de generación (`AAAA-MM-DD`). |

**Example response 200 OK**

```json
{
  "dTOqrDetails": [
    {
      "id": 49,
      "amount": 20,
      "currency": "BOB",
      "generationDate": "2026-11-18T19:44:01",
      "expirationDate": "2026-11-19T00:00:00",
      "statusId": 1,
      "transactionDate": "0001-01-01T00:00:00",
      "sourceBank": null,
      "gloss": "Prueba QR",
      "destinationAccountNumber": 1,
      "notificationSuccess": false,
      "notificationResponse": null,
      "voucherId": null
    },
    {
      "id": 50,
      "amount": 25.5,
      "currency": "BOB",
      "generationDate": "2026-11-18T21:12:40",
      "expirationDate": "2026-11-19T00:00:00",
      "statusId": 2,
      "transactionDate": "2026-11-18T21:13:02",
      "sourceBank": "BANCO VALMED",
      "gloss": "Entrada A-014",
      "destinationAccountNumber": 1,
      "notificationSuccess": true,
      "notificationResponse": "{\"success\": true, \"message\": \"OK\"}",
      "voucherId": "CWMYKR9YD1"
    }
  ],
  "success": true,
  "message": null
}
```

| Key | Description |
|---|---|
| dTOqrDetails | Lista de QR de ese día (el nombre es así, con esas mayúsculas). |
| id | Identificador del QR (número). |
| amount | Monto, como número. |
| currency | `BOB`. |
| generationDate | Cuándo se generó (hora de Bolivia, sin zona). |
| expirationDate | Cuándo vence. |
| statusId | 1 = no usado, 2 = usado, 3 = expirado, 4 = con error (en ValMed: anulado). |
| transactionDate | Cuándo se pagó; `0001-01-01T00:00:00` si no se usó. |
| sourceBank | Banco desde el que se pagó; `null` si no se usó. |
| gloss | El `gloss` del QR. |
| destinationAccountNumber | Posición de la cuenta de abono (1). |
| notificationSuccess | `true` si tu servidor respondió `{"success": true}` a la notificación. |
| notificationResponse | Lo que respondió tu servidor. |
| voucherId | Código de la transacción si se pagó. |

## 09. Obtener estado de un QR

`POST /QRSimple.API/api/v1/main/getQRStatusAsync`

**Body**

| Key | Value | Description |
|---|---|---|
| qrId | `59` | El `id` que devolvió [§07](#07-generar-qr). |

**Example request**

```bash
curl -X POST "http://localhost:8086/QRSimple.API/api/v1/main/getQRStatusAsync" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"qrId":59}'
```

**Example response 200 OK**

```json
{
  "id": 59,
  "statusId": 2,
  "expirationDate": "2026-11-19T00:00:00",
  "voucherId": "3JAO702813",
  "success": true,
  "message": null
}
```

| Key | Description |
|---|---|
| id | Identificador del QR consultado. |
| statusId | 1 = no usado, 2 = usado, 3 = expirado, 4 = con error (en ValMed: anulado). |
| expirationDate | Cuándo vence. |
| voucherId | Código de la transacción si se pagó. Si no, `null`. El BNB dice que puede llegar `"0"`: trata `null` y `"0"` como «sin pago». |
| success / message | `false` y `"QR no encontrado"` si el QR no existe o es de otra cuenta. |

## 10. Cancelar QR

`POST /QRSimple.API/api/v1/main/CancelQRByIdAsync`

Anula un QR **antes de que alguien lo pague**. Anular uno ya anulado no es error.

**Body**: `{"qrId": 59}`

**Example response 200 OK**

```json
{ "success": true, "message": null }
```

Si el QR ya se pagó o venció, responde `success: false` con el motivo (`"El QR ya fue usado: no se puede anular"`).

## 11. Notificación de pagos QR (ReceiveNotification)

Cuando alguien paga un QR generado con esta API, el banco hace `POST` a la URL de notificación de tu clave. El BNB la llama `ReceiveNotification`.

**Request que recibe tu sistema**

```http
POST /pagos/ReceiveNotification HTTP/1.1
Content-Type: application/json

{
  "QRId": "76",
  "Gloss": "Entrada A-014",
  "sourceBankId": 1,
  "originName": "ROSA QUISPE",
  "VoucherId": "1J9F121212",
  "TransactionDateTime": "18/11/2026 17:30:15",
  "additionalData": "orden-6f1c2a9e"
}
```

| Key | Description |
|---|---|
| QRId | El `id` que devolvió `getQRWithImageAsync` (texto). |
| Gloss | El `gloss` con el que se generó el QR. |
| sourceBankId | Entidad financiera desde la que se pagó. En ValMed siempre es `1`. |
| originName | Titular de la cuenta que pagó (en mayúsculas). |
| VoucherId | Código de la transacción: 10 caracteres alfanuméricos. |
| TransactionDateTime | Fecha y hora del pago: `dd/MM/yyyy HH:mm:ss`, hora de Bolivia. |
| additionalData | El `additionalData` del QR (`""` si no se mandó). |

**Response que debe dar tu sistema**

```json
{ "success": true, "message": "OK" }
```

Si respondes otra cosa (`success: false`, un cuerpo sin JSON, un 5xx o nada), ValMed **reintenta** a los 5 s, 15 s, 1 min, 5 min, 15 min y 1 h: 7 intentos en total. El resultado se ve en `notificationSuccess` ([§08](#08-obtener-lista-de-qr-generados)) y en la web app, **Desarrolladores → Avisos enviados**, donde también está el botón «Reenviar».

**Cómo procesarla bien (igual para ValMed y el BNB)**

1. Busca tu orden por `QRId`. Si no existe, responde `{"success": true}` igual: no es tuya y no quieres reintentos.
2. **Confirma con `getQRStatusAsync`** que `statusId` sea `2`. **La notificación del BNB no va firmada**: no marques nada como pagado solo porque llegó un POST.
3. Si la orden ya estaba pagada, no hagas nada más: la notificación **puede llegar más de una vez**. Usa `QRId` o `VoucherId` como llave para no procesarla dos veces.
4. Guarda `VoucherId` como comprobante y responde `{"success": true, "message": "OK"}` rápido (en menos de 5 s).

> ValMed agrega las cabeceras `X-ValMed-Event-Id`, `X-ValMed-Timestamp` y `X-ValMed-Signature` (HMAC-SHA256, ver la [guía de la API propia](GUIA_INTEGRACION.md#8-avisos-firmados-webhook)). Son opcionales: el BNB no las manda, así que si quieres migrar, no dependas de ellas. El paso 2 da la misma seguridad en los dos bancos.

### Ejemplo en Java (Spring Boot)

```java
// application.yml (cambia solo esto para pasar al BNB)
// banco.auth-url: http://banco:8080/ClientAuthentication.API/api/v1
// banco.qr-url:   http://banco:8080/QRSimple.API/api/v1
// banco.account-id / banco.authorization-id: desde variables de entorno

@Component
public class BancoQrClient {
    private final RestClient http = RestClient.create();
    private final BancoProps props;
    private volatile String token;

    public BancoQrClient(BancoProps props) { this.props = props; }

    private synchronized String token(boolean renovar) {
        if (token == null || renovar) {
            Map<?, ?> r = http.post().uri(props.authUrl() + "/auth/token")
                .contentType(MediaType.APPLICATION_JSON)
                .body(Map.of("accountId", props.accountId(), "authorizationId", props.authorizationId()))
                .retrieve().body(Map.class);
            if (!Boolean.TRUE.equals(r.get("success"))) throw new IllegalStateException("Banco: " + r.get("message"));
            token = (String) r.get("message");
        }
        return token;
    }

    /** POST a QRSimple.API con el token; si responde 401, renueva el token y repite una vez. */
    Map<?, ?> llamar(String servicio, Map<String, ?> cuerpo) {
        for (int intento = 0; ; intento++) {
            try {
                Map<?, ?> r = http.post().uri(props.qrUrl() + "/main/" + servicio)
                    .header("Authorization", "Bearer " + token(intento > 0))
                    .contentType(MediaType.APPLICATION_JSON).body(cuerpo)
                    .retrieve().body(Map.class);
                if (!Boolean.TRUE.equals(r.get("success"))) throw new IllegalStateException("Banco: " + r.get("message"));
                return r;
            } catch (HttpClientErrorException.Unauthorized e) {
                if (intento > 0) throw e;
            }
        }
    }

    public Map<?, ?> generarQr(String glosa, BigDecimal monto, LocalDate ultimoDia, String ordenId) {
        return llamar("getQRWithImageAsync", Map.of("currency", "BOB", "gloss", glosa, "amount", monto,
            "singleUse", true, "expirationDate", ultimoDia.toString(), "additionalData", ordenId));
    }

    public boolean pagado(String qrId) {
        return ((Number) llamar("getQRStatusAsync", Map.of("qrId", qrId)).get("statusId")).intValue() == 2;
    }
}

@RestController
class NotificacionBancoController {
    private final BancoQrClient banco;
    private final OrdenService ordenes;
    NotificacionBancoController(BancoQrClient banco, OrdenService ordenes) { this.banco = banco; this.ordenes = ordenes; }

    @PostMapping("/pagos/ReceiveNotification")
    Map<String, Object> recibir(@RequestBody Map<String, Object> n) {
        String qrId = String.valueOf(n.get("QRId"));
        ordenes.buscarPorQrId(qrId)
            .filter(o -> !o.estaPagada())
            .filter(o -> banco.pagado(qrId))              // confirma con el banco
            .ifPresent(o -> ordenes.marcarPagada(o, (String) n.get("VoucherId")));
        return Map.of("success", true, "message", "OK");
    }
}
```

---

## 12. Qué datos entrega el QR

### Dentro de la imagen

| | Banco ValMed | BNB |
|---|---|---|
| Contenido | Una URL `http://…/?p=<id interno>` que abre la web app de ValMed | El QR interoperable «Simple», que lee la app de cualquier banco de Bolivia |
| Quién lo puede pagar | Solo clientes de ValMed (es un banco simulado) | Clientes de cualquier banco |

**Regla:** trata `qr` como una imagen opaca. No la decodifiques ni armes QR por tu cuenta: el contenido cambia de un banco a otro.

### En cada momento

| Dato | Al generar (`getQRWithImageAsync`) | Notificación (`ReceiveNotification`) | `getQRStatusAsync` | `getQRbyGenerationDateAsync` |
|---|---|---|---|---|
| Id del QR | `id` | `QRId` | `id` | `id` |
| Imagen | `qr` | — | — | — |
| Estado | — | (implícito: pagado) | `statusId` | `statusId` |
| Monto y moneda | — | **no** | **no** | `amount`, `currency` |
| Glosa | — | `Gloss` | — | `gloss` |
| `additionalData` | — | `additionalData` | — | **no** |
| Vencimiento | — | — | `expirationDate` | `expirationDate` |
| Cuándo se pagó | — | `TransactionDateTime` | — | `transactionDate` |
| Código de la transacción | — | `VoucherId` | `voucherId` | `voucherId` |
| Banco de quien pagó | — | `sourceBankId` | — | `sourceBank` |
| **Nombre** de quien pagó | — | `originName` | — | — |
| **C.I. o cuenta** de quien pagó | — | **no** | **no** | **no** |
| Resultado de la notificación | — | — | — | `notificationSuccess`, `notificationResponse` |

### Qué implica para quien integra

- **No hay C.I. de quien pagó, solo el nombre (`originName`).** No se puede comprobar que pagó una persona en particular, por ejemplo el dueño del turno o de la reserva. Si tu flujo lo necesita, decide qué hacer cuando el nombre no coincide: con el BNB no hay otro dato. La API propia de ValMed sí da el C.I., pero ese dato **no** existe al migrar.
- **La notificación no trae el monto.** Guarda el monto en tu orden al generar el QR. Como el QR es de un solo pago y de monto fijo, lo pagado siempre es lo cobrado.
- **El QR vence por día, no por minutos.** Si reservas algo por 10 minutos, el vencimiento lo controla **tu sistema**:
  1. Al terminar la reserva, llama a `CancelQRByIdAsync`.
  2. Si responde `success: false` porque ya se usó, **sí pagaron**: respeta el pago o devuélvelo.
  3. Entre el pago y la notificación pasan unos segundos. Antes de liberar la reserva, consulta `getQRStatusAsync`.
- **La API Market no tiene devoluciones.** En el BNB, devolver un pago se hace fuera de esta API. En ValMed puedes probarlo con la API propia (`POST /api/v1/payments/{bankReference}/refunds`), pero eso no migra.
- **No hay eventos en vivo.** Tu front pregunta a **tu** backend cada 3 a 5 s. Tu backend se entera por la notificación, o consultando `getQRStatusAsync` si la notificación tarda.

## 13. Diferencias con el BNB

| Tema | Banco ValMed | BNB |
|---|---|---|
| Dinero | Simulado | Real |
| Credenciales | Las creas tú en la web app | Las entrega el BNB tras firmar el convenio |
| `UpdateCredentials` antes del primer uso | Opcional | Obligatorio |
| Vida del token | 1 hora (JWT HS256) | No documentada: renueva ante un 401 |
| `singleUse: false` (QR para varios pagos) | No: responde `success: false` | Sí |
| `currency: USD` / `destinationAccountId: 2` | No: responde `success: false` | Sí |
| `statusId` tras anular | 4 | No documentado |
| `voucherId` sin pago | `null` | `null` en los ejemplos; `"0"` según el texto |
| Códigos HTTP de error | 200 + `success: false`, 400, 401, 5xx (ver [§04](#04-diseño)) | Solo documenta 200: revisa siempre `success` |
| Firma de la notificación | Cabeceras `X-ValMed-*` (opcionales) | Ninguna |
| Reintentos de la notificación | 7 intentos en ~1 h 20 min, hasta recibir `success: true` | No documentados |
| `sourceBankId` | Siempre 1 | Código del banco de quien pagó |
| Rutas | Distingue mayúsculas: escríbelas como en este documento | No las distingue |
| Pagar sin celular | `POST /api/v1/sandbox/qr/{qrId}/pay` ([§15](#15-pruebas-sin-celular-solo-valmed)) | No existe: se paga con una app real |
| Límites | Monto ≤ 100 000; `gloss` ≤ 140; `additionalData` ≤ 500; vencimiento ≤ 365 días | No documentados |

## 14. Migrar al BNB

Si tu sistema solo usa lo descrito en las secciones 03 a 11:

1. Firma el convenio con el BNB y pide credenciales de **prueba** (`accountId`, `authorizationId`).
2. Cambia el `authorizationId` con `UpdateCredentials` (obligatorio en el BNB).
3. Cambia la configuración:

   | Variable (ejemplo) | Banco ValMed | BNB pruebas |
   |---|---|---|
   | `BANCO_AUTH_URL` | `http://banco:8080/ClientAuthentication.API/api/v1` | `http://test.bnb.com.bo/ClientAuthentication.API/api/v1` |
   | `BANCO_QR_URL` | `http://banco:8080/QRSimple.API/api/v1` | `http://test.bnb.com.bo/QRSimple.API/api/v1` |
   | `BANCO_ACCOUNT_ID` / `BANCO_AUTHORIZATION_ID` | Los de la web app | Los del BNB |

   Usa dos URL separadas: el sandbox del portal del BNB publica la autenticación y los QR en servidores distintos.
4. Da al BNB tu URL de notificación: debe ser **pública y con HTTPS**.
5. Quita lo que es solo de ValMed: el sandbox, la API propia (`/api/v1/...`) y la verificación de `X-ValMed-Signature`.
6. Prueba en el ambiente de pruebas del BNB y luego pasa a producción con las credenciales y la URL que te dé el BNB.

## 15. Pruebas sin celular (solo ValMed)

Para probar el flujo completo sin escanear con un celular:

```bash
curl -X POST "http://localhost:8086/api/v1/sandbox/qr/51/pay" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"payerCi":"7000001"}'
```

- `51` es el `id` del QR. También acepta la cabecera `X-Api-Key` en lugar del token.
- Paga desde la cuenta de ese C.I.; si no existe, la crea con Bs 1 000. Las cuentas de prueba son 7000001 Rosa (Bs 2 000), 7000002 Juan (Bs 50), 7000003 Luis y 7000004 Carla, todas con PIN `123456`.
- Dispara la misma notificación que un pago real. Con `7000002` puedes probar **saldo insuficiente**.
- Solo funciona con `BANK_SANDBOX=true`. **No existe en el BNB.**

También puedes pagar «de verdad» desde la web app (`http://localhost:8086`): entra con una cuenta de prueba, elige **Pagar QR** y escanea el QR, sube una foto de él o escribe su código.
