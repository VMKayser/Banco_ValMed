-- Banco ValMed: esquema inicial. Dinero en numeric(14,2), moneda única BOB.

CREATE SEQUENCE account_number_seq START 1;
CREATE SEQUENCE bank_reference_seq START 1;

-- Clientes del banco (personas o comercios: el banco no los distingue).
CREATE TABLE customers (
    id                  uuid PRIMARY KEY,
    ci_number           varchar(10)  NOT NULL,
    ci_complement       varchar(2),
    full_name           varchar(120) NOT NULL,
    email               varchar(254),
    google_sub          varchar(255) UNIQUE,
    pin_hash            text         NOT NULL,
    failed_pin_attempts int          NOT NULL DEFAULT 0,
    locked_until        timestamptz,
    created_at          timestamptz  NOT NULL DEFAULT now(),
    CONSTRAINT uk_customers_ci UNIQUE NULLS NOT DISTINCT (ci_number, ci_complement)
);

-- Una cuenta por cliente. El saldo nunca baja de 0.
CREATE TABLE accounts (
    id          uuid PRIMARY KEY,
    customer_id uuid          NOT NULL UNIQUE REFERENCES customers (id),
    number      varchar(11)   NOT NULL UNIQUE,
    currency    char(3)       NOT NULL DEFAULT 'BOB',
    balance     numeric(14,2) NOT NULL DEFAULT 0 CHECK (balance >= 0),
    created_at  timestamptz   NOT NULL DEFAULT now()
);

-- Sesiones de la app: token opaco, se guarda solo su SHA-256.
CREATE TABLE sessions (
    id           uuid PRIMARY KEY,
    customer_id  uuid        NOT NULL REFERENCES customers (id),
    token_hash   bytea       NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz NOT NULL,
    revoked_at   timestamptz
);

-- Claves de API de integradores (otro sistema que cobra con QR).
CREATE TABLE api_keys (
    id             uuid PRIMARY KEY,
    customer_id    uuid        NOT NULL REFERENCES customers (id),
    name           varchar(60) NOT NULL,
    prefix         varchar(20) NOT NULL,
    key_hash       bytea       NOT NULL UNIQUE,
    webhook_url    text,
    webhook_secret text        NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    revoked_at     timestamptz
);

-- Cobros QR dinámicos: monto fijo, un solo uso, con vencimiento.
-- EXPIRED no se guarda: es ACTIVE con expires_at vencido.
CREATE TABLE qr_charges (
    id                 uuid PRIMARY KEY,
    code               char(8)       NOT NULL,
    account_id         uuid          NOT NULL REFERENCES accounts (id),
    api_key_id         uuid          REFERENCES api_keys (id),
    merchant_reference varchar(100),
    amount             numeric(14,2) NOT NULL CHECK (amount > 0),
    currency           char(3)       NOT NULL DEFAULT 'BOB',
    description        varchar(140)  NOT NULL,
    metadata           jsonb,
    status             varchar(10)   NOT NULL DEFAULT 'ACTIVE'
                       CHECK (status IN ('ACTIVE', 'PAID', 'CANCELLED', 'REFUNDED')),
    expires_at         timestamptz   NOT NULL,
    created_at         timestamptz   NOT NULL DEFAULT now(),
    paid_at            timestamptz,
    cancelled_at       timestamptz,
    refunded_at        timestamptz,
    CONSTRAINT uk_qr_code UNIQUE (code)
);
CREATE UNIQUE INDEX uk_qr_reference ON qr_charges (account_id, merchant_reference)
    WHERE merchant_reference IS NOT NULL;
CREATE INDEX ix_qr_account ON qr_charges (account_id, created_at DESC);

-- UNIQUE(qr_charge_id): un QR se paga una sola vez.
CREATE TABLE payments (
    id               uuid PRIMARY KEY,
    bank_reference   varchar(20)   NOT NULL UNIQUE,
    qr_charge_id     uuid          NOT NULL UNIQUE REFERENCES qr_charges (id),
    payer_account_id uuid          NOT NULL REFERENCES accounts (id),
    amount           numeric(14,2) NOT NULL,
    paid_at          timestamptz   NOT NULL DEFAULT now(),
    refunded_at      timestamptz
);

-- Solo devoluciones completas: una por pago.
CREATE TABLE refunds (
    id         uuid PRIMARY KEY,
    payment_id uuid          NOT NULL UNIQUE REFERENCES payments (id),
    reason     varchar(40)   NOT NULL,
    amount     numeric(14,2) NOT NULL,
    created_at timestamptz   NOT NULL DEFAULT now()
);

-- Libro mayor: cada movimiento de saldo deja una fila con el saldo resultante.
CREATE TABLE ledger_entries (
    id            bigserial PRIMARY KEY,
    account_id    uuid          NOT NULL REFERENCES accounts (id),
    kind          varchar(20)   NOT NULL,
    amount        numeric(14,2) NOT NULL,
    balance_after numeric(14,2) NOT NULL,
    description   varchar(160)  NOT NULL,
    counterparty  varchar(160),
    payment_id    uuid REFERENCES payments (id),
    refund_id     uuid REFERENCES refunds (id),
    created_at    timestamptz   NOT NULL DEFAULT now()
);
CREATE INDEX ix_ledger_account ON ledger_entries (account_id, id DESC);

-- Outbox de avisos firmados: la fila nace en la misma transacción que el pago.
CREATE TABLE webhook_deliveries (
    id               uuid PRIMARY KEY,
    api_key_id       uuid        NOT NULL REFERENCES api_keys (id),
    qr_charge_id     uuid        NOT NULL REFERENCES qr_charges (id),
    event_type       varchar(40) NOT NULL,
    url              text        NOT NULL,
    body             text        NOT NULL,
    status           varchar(10) NOT NULL DEFAULT 'PENDING'
                     CHECK (status IN ('PENDING', 'DELIVERED', 'FAILED')),
    attempts         int         NOT NULL DEFAULT 0,
    next_attempt_at  timestamptz NOT NULL DEFAULT now(),
    last_status_code int,
    last_error       text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    delivered_at     timestamptz
);
CREATE INDEX ix_webhook_pending ON webhook_deliveries (next_attempt_at) WHERE status = 'PENDING';
CREATE INDEX ix_webhook_key ON webhook_deliveries (api_key_id, created_at DESC);
