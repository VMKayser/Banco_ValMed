-- Compatibilidad con la API Market (Pago QR) del Banco Nacional de Bolivia:
-- credenciales accountId/authorizationId, id numérico del QR, código de
-- transacción de 10 caracteres y notificaciones con el formato del BNB.

-- accountId es público; authorizationId es secreto (se guarda su SHA-256). Al
-- crear la clave, authorizationId es la misma clave de API; UpdateCredentials
-- lo cambia sin tocar X-Api-Key.
ALTER TABLE api_keys
    ADD COLUMN market_account_id  varchar(44),
    ADD COLUMN market_secret_hash bytea;
UPDATE api_keys
   SET market_account_id  = encode(decode(replace(gen_random_uuid()::text, '-', ''), 'hex'), 'base64'),
       market_secret_hash = key_hash;
ALTER TABLE api_keys
    ALTER COLUMN market_account_id SET NOT NULL,
    ALTER COLUMN market_secret_hash SET NOT NULL,
    ADD CONSTRAINT uk_api_keys_market_account UNIQUE (market_account_id);

-- number: id numérico del QR (el "id"/"qrId" del BNB).
-- channel: por dónde se creó; los de API_MARKET notifican con el formato del BNB.
ALTER TABLE qr_charges
    ADD COLUMN number          bigint GENERATED ALWAYS AS IDENTITY,
    ADD COLUMN channel         varchar(12) NOT NULL DEFAULT 'API'
                               CHECK (channel IN ('APP', 'API', 'API_MARKET')),
    ADD COLUMN additional_data varchar(500);
ALTER TABLE qr_charges ADD CONSTRAINT uk_qr_number UNIQUE (number);
UPDATE qr_charges SET channel = 'APP' WHERE api_key_id IS NULL;

-- voucher_id: "código de bancarización" de 10 caracteres (VoucherId del BNB).
ALTER TABLE payments ADD COLUMN voucher_id char(10);
UPDATE payments SET voucher_id = upper(substr(md5(id::text), 1, 10));
ALTER TABLE payments
    ALTER COLUMN voucher_id SET NOT NULL,
    ADD CONSTRAINT uk_payments_voucher UNIQUE (voucher_id);

-- format: VALMED (evento firmado) o API_MARKET (ReceiveNotification del BNB).
-- last_response: lo que respondió el integrador (notificationResponse).
ALTER TABLE webhook_deliveries
    ADD COLUMN format        varchar(12) NOT NULL DEFAULT 'VALMED'
                             CHECK (format IN ('VALMED', 'API_MARKET')),
    ADD COLUMN last_response varchar(500);
