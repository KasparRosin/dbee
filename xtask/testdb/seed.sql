-- Seed for the dbee test servers (`cargo xtask testdb up`), the same on every Postgres major.
-- Synthetic data only. The entrypoint runs it once, into database `dbee`, before the server
-- accepts TCP connections. Tests treat it as read-only; tests that write use schema `scratch`.

-- ---------------------------------------------------------------------------------------------
-- shop: a small order system with the types, keys and sizes the client has to cope with
-- ---------------------------------------------------------------------------------------------
CREATE SCHEMA shop;

CREATE TYPE shop.order_status AS ENUM ('new', 'paid', 'completed', 'cancelled');
CREATE DOMAIN shop.json_doc AS jsonb;
CREATE TYPE shop.pair AS (a int, b text);

CREATE TABLE shop.customers (
    id serial PRIMARY KEY,
    name text NOT NULL,
    email varchar(200) UNIQUE,
    created_at timestamptz DEFAULT now(),
    note text
);

INSERT INTO shop.customers (name, email, created_at, note)
SELECT 'Customer ' || n,
       'customer' || n || '@example.com',
       timestamptz '2025-01-01 09:00:00+00' + n * interval '7 hours',
       CASE WHEN n % 5 = 0 THEN NULL ELSE 'note for customer ' || n END
FROM generate_series(1, 500) AS n
ORDER BY n;

CREATE TABLE shop.orders (
    id bigserial PRIMARY KEY,
    customer_id int REFERENCES shop.customers,
    dealer_id int,
    status shop.order_status,
    total numeric(12,2),
    billing jsonb,
    meta json,
    shipped_at date,
    raw_json_text text
);

-- Dealer 42 has completed orders above 100 (the filter example): dealer and status cycle with
-- different periods, so every combination occurs.
INSERT INTO shop.orders (customer_id, dealer_id, status, total, billing, meta, shipped_at, raw_json_text)
SELECT 1 + (n - 1) % 500,
       CASE WHEN n % 97 = 0 THEN NULL ELSE 1 + n % 50 END,
       s.status,
       round(((n * 7919) % 50000) / 100.0 + 1, 2),
       jsonb_build_object(
           'name', 'Customer ' || (1 + (n - 1) % 500),
           'address', jsonb_build_object(
               'street', n || ' Example Street',
               'city', 'City ' || (n % 20),
               'zip', lpad((n % 100000)::text, 5, '0'),
               'geo', jsonb_build_object('lat', round(59 + (n % 100) / 100.0, 2), 'lon', round(24 + (n % 100) / 100.0, 2))),
           'items', jsonb_build_array(
               jsonb_build_object('sku', 'SKU-' || n, 'qty', 1 + n % 3),
               jsonb_build_object('sku', 'SKU-' || (n + 1), 'qty', 1)),
           'paid', s.status IN ('paid', 'completed')),
       to_json(format('{"a":%s}', 1 + (n - 1) % 3)),
       CASE WHEN s.status IN ('completed', 'cancelled') THEN date '2025-01-01' + n % 600 END,
       '{"k":[1,2]}'
FROM generate_series(1, 5000) AS n
CROSS JOIN LATERAL (
    SELECT (ARRAY['new', 'paid', 'completed', 'cancelled']::shop.order_status[])[1 + (n / 3) % 4] AS status
) AS s
ORDER BY n;

CREATE TABLE shop.order_items (
    order_id bigint REFERENCES shop.orders,
    line_no int,
    product text,
    qty int,
    PRIMARY KEY (order_id, line_no)
);

INSERT INTO shop.order_items (order_id, line_no, product, qty)
SELECT o.id, l, 'Product ' || ((o.id * 31 + l) % 200), 1 + (o.id + l) % 5
FROM shop.orders AS o
CROSS JOIN LATERAL generate_series(1, 1 + (o.id % 3)::int) AS l
ORDER BY o.id, l;

CREATE TABLE shop.shipments (
    id serial PRIMARY KEY,
    order_id bigint,
    line_no int,
    FOREIGN KEY (order_id, line_no) REFERENCES shop.order_items
);

INSERT INTO shop.shipments (order_id, line_no)
SELECT order_id, line_no FROM shop.order_items WHERE line_no = 1 AND order_id % 2 = 0 ORDER BY order_id;

CREATE TABLE shop.all_types (
    id serial PRIMARY KEY,
    note text,
    st shop.order_status,
    doc shop.json_doc,
    r int4range,
    iv interval,
    m money,
    n numeric(30,10),
    ts timestamptz,
    b bytea,
    u uuid,
    ip inet,
    arr text[],
    pt point,
    comp shop.pair
);

INSERT INTO shop.all_types (note, st, doc, r, iv, m, n, ts, b, u, ip, arr, pt, comp) VALUES
    ('first row', 'paid', '{"a": {"b": [1, 2, 3]}, "s": "text"}', '[1,10)', '1 day 02:03:04', 12.34,
     12345678901234567890.0123456789, '2025-06-01 12:34:56.789+00', '\xdeadbeef',
     'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '192.168.0.1/24', ARRAY['x', 'y z', 'quote"d', NULL],
     '(1.5,-2)', ROW(1, 'one')),
    ('second row: edge values', 'cancelled', '[]', 'empty', '-1 year -2 mons +3 days -04:05:06', -0.01,
     -0.0000000001, '1999-12-31 23:59:59+00', '\x', '00000000-0000-0000-0000-000000000000', '::1',
     ARRAY[]::text[], '(0,0)', ROW(NULL, 'with "quotes", commas')),
    (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

CREATE TABLE shop.no_key (a int, b text);
INSERT INTO shop.no_key (a, b) VALUES (1, 'one'), (1, 'one'), (2, 'two'), (NULL, NULL);

CREATE VIEW shop.order_totals AS
    SELECT customer_id, count(*) AS orders, sum(total) AS total
    FROM shop.orders
    GROUP BY customer_id;

CREATE MATERIALIZED VIEW shop.customer_counts AS
    SELECT c.id AS customer_id, c.name, count(o.id) AS orders
    FROM shop.customers AS c
    LEFT JOIN shop.orders AS o ON o.customer_id = c.id
    GROUP BY c.id, c.name;

CREATE TABLE shop.events (id int, at date) PARTITION BY RANGE (at);
CREATE TABLE shop.events_2025 PARTITION OF shop.events FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
CREATE TABLE shop.events_2026 PARTITION OF shop.events FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
INSERT INTO shop.events (id, at)
SELECT n, date '2025-01-01' + n * 3 FROM generate_series(1, 200) AS n;

CREATE TABLE shop.big_rows (id int PRIMARY KEY, payload text);
INSERT INTO shop.big_rows (id, payload)
SELECT n, 'payload ' || n FROM generate_series(1, 1000000) AS n;

-- One row with a 20,000,000-character text and a jsonb of about the same size as text.
CREATE TABLE shop.big_cell (id int PRIMARY KEY, t text, j jsonb);
INSERT INTO shop.big_cell (id, t, j)
SELECT 1,
       repeat('0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_', 312500),
       (SELECT jsonb_agg(jsonb_build_object('i', i, 's', repeat('z', 40))) FROM generate_series(1, 320000) AS i);

-- ---------------------------------------------------------------------------------------------
-- many: 300 small tables, t_001 ... t_300
-- ---------------------------------------------------------------------------------------------
CREATE SCHEMA many;

DO $$
BEGIN
    FOR i IN 1..300 LOOP
        EXECUTE format('CREATE TABLE many.%I (id int PRIMARY KEY, v text)', 't_' || lpad(i::text, 3, '0'));
        EXECUTE format('INSERT INTO many.%I (id, v) VALUES (1, %L)', 't_' || lpad(i::text, 3, '0'), 'row of table ' || i);
    END LOOP;
END
$$;

-- ---------------------------------------------------------------------------------------------
-- odd: identifiers that need quoting (quotes, spaces, upper case, non-ASCII, emoji)
-- ---------------------------------------------------------------------------------------------
CREATE SCHEMA odd;

CREATE TABLE odd."Other Ödd" (
    id int PRIMARY KEY,
    "Label" text
);
INSERT INTO odd."Other Ödd" (id, "Label") VALUES (1, 'first'), (2, 'second'), (3, 'third');

CREATE TABLE odd."Odd ""Name"" 🐙" (
    id int PRIMARY KEY,
    "Total $" numeric,
    "lower UPPER" text,
    "Other Ödd id" int REFERENCES odd."Other Ödd" (id)
);
INSERT INTO odd."Odd ""Name"" 🐙" (id, "Total $", "lower UPPER", "Other Ödd id") VALUES
    (1, 0.5, 'alpha', 1),
    (2, 12.75, 'Beta', 2),
    (3, 100, 'gamma 🐙', 3),
    (4, NULL, NULL, NULL);

-- ---------------------------------------------------------------------------------------------
-- scratch: empty, for tests that write (each test creates and drops its own table)
-- ---------------------------------------------------------------------------------------------
CREATE SCHEMA scratch;

ANALYZE;
