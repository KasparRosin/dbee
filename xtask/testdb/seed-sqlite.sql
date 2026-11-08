-- Seed for dbee's SQLite tests (`dbee_testkit::sqlite_seeded`): the shapes of seed.sql (Postgres)
-- in SQLite's dialect. Synthetic data only. dbee_testkit builds it into a file once and hands each
-- test a copy of its own, so a test may write to its copy.
--
-- SQLite has one schema, `main`: shop's tables keep their names, many's are t_001 ... t_300 and
-- odd's keep their quoted names. There is no `scratch`, as every test has its own copy. What SQLite
-- lacks gets the nearest shape: the enum is a CHECK, the materialized view a view, the partitioned
-- table a plain table, JSON is text. SQLite-only shapes: `notes_nokey`, a table without a key whose
-- rows dbee keys by `rowid`, and `rowid_taken`, a table with a column named `rowid`.

-- Sorts and CTEs stay in memory, not in the system temp directory.
PRAGMA temp_store = MEMORY;

BEGIN;

-- ---------------------------------------------------------------------------------------------
-- shop: a small order system with the types, keys and sizes the client has to cope with
-- ---------------------------------------------------------------------------------------------
CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    email VARCHAR(200) UNIQUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    note TEXT
);

INSERT INTO customers (id, name, email, created_at, note)
WITH RECURSIVE n(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM n WHERE n < 500)
SELECT n,
       'Customer ' || n,
       'customer' || n || '@example.com',
       datetime('2025-01-01 09:00:00', '+' || (n * 7) || ' hours'),
       CASE WHEN n % 5 = 0 THEN NULL ELSE 'note for customer ' || n END
FROM n;

CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER REFERENCES customers (id),
    dealer_id INTEGER,
    status TEXT CHECK (status IN ('new', 'paid', 'completed', 'cancelled')),
    total NUMERIC(12,2),
    billing JSON,
    meta JSON,
    shipped_at DATE,
    raw_json_text TEXT
);

-- Dealer 42 has completed orders above 100 (the filter example): dealer and status cycle with
-- different periods, so every combination occurs. `meta` holds a JSON string, as in seed.sql.
INSERT INTO orders (id, customer_id, dealer_id, status, total, billing, meta, shipped_at, raw_json_text)
WITH RECURSIVE n(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM n WHERE n < 5000),
s AS (
    SELECT n, CASE (n / 3) % 4 WHEN 0 THEN 'new' WHEN 1 THEN 'paid' WHEN 2 THEN 'completed' ELSE 'cancelled' END AS status
    FROM n
)
SELECT n,
       1 + (n - 1) % 500,
       CASE WHEN n % 97 = 0 THEN NULL ELSE 1 + n % 50 END,
       status,
       round(((n * 7919) % 50000) / 100.0 + 1, 2),
       json_object(
           'name', 'Customer ' || (1 + (n - 1) % 500),
           'address', json_object(
               'street', n || ' Example Street',
               'city', 'City ' || (n % 20),
               'zip', printf('%05d', n % 100000),
               'geo', json_object('lat', round(59 + (n % 100) / 100.0, 2), 'lon', round(24 + (n % 100) / 100.0, 2))),
           'items', json_array(
               json_object('sku', 'SKU-' || n, 'qty', 1 + n % 3),
               json_object('sku', 'SKU-' || (n + 1), 'qty', 1)),
           'paid', json(CASE WHEN status IN ('paid', 'completed') THEN 'true' ELSE 'false' END)),
       json_quote('{"a":' || (1 + (n - 1) % 3) || '}'),
       CASE WHEN status IN ('completed', 'cancelled') THEN date('2025-01-01', '+' || (n % 600) || ' days') END,
       '{"k":[1,2]}'
FROM s
ORDER BY n;

CREATE TABLE order_items (
    order_id INTEGER REFERENCES orders (id),
    line_no INTEGER,
    product TEXT,
    qty INTEGER,
    PRIMARY KEY (order_id, line_no)
);

INSERT INTO order_items (order_id, line_no, product, qty)
WITH RECURSIVE l(l) AS (SELECT 1 UNION ALL SELECT l + 1 FROM l WHERE l < 3)
SELECT o.id, l.l, 'Product ' || ((o.id * 31 + l.l) % 200), 1 + (o.id + l.l) % 5
FROM orders AS o
JOIN l ON l.l <= 1 + o.id % 3
ORDER BY o.id, l.l;

CREATE TABLE shipments (
    id INTEGER PRIMARY KEY,
    order_id INTEGER,
    line_no INTEGER,
    FOREIGN KEY (order_id, line_no) REFERENCES order_items (order_id, line_no)
);

-- The ids seed.sql's serial gives: the n-th shipment is for order 2n.
INSERT INTO shipments (id, order_id, line_no)
SELECT order_id / 2, order_id, line_no FROM order_items WHERE line_no = 1 AND order_id % 2 = 0 ORDER BY order_id;

-- One column per affinity and per declared type dbee tells apart: INTEGER, REAL, NUMERIC, TEXT,
-- BLOB and no type; BOOLEAN, JSON, DATE and DATETIME; and a generated column.
CREATE TABLE all_types (
    id INTEGER PRIMARY KEY,
    note TEXT,
    st TEXT CHECK (st IN ('new', 'paid', 'completed', 'cancelled')),
    doc JSON,
    flag BOOLEAN,
    i INTEGER,
    r REAL,
    m NUMERIC,
    n DECIMAL(30,10),
    d DATE,
    ts DATETIME,
    b BLOB,
    u VARCHAR(36),
    anything,
    total REAL GENERATED ALWAYS AS (r * 2) VIRTUAL
);

INSERT INTO all_types (id, note, st, doc, flag, i, r, m, n, d, ts, b, u, anything) VALUES
    (1, 'first row', 'paid', '{"a": {"b": [1, 2, 3]}, "s": "text"}', 1, 42, 3.141592653589793, 12.34,
     12345678901234567890.0123456789, '2025-06-01', '2025-06-01 12:34:56.789', X'DEADBEEF',
     'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', 'text in a column without a type'),
    (2, 'second row: edge values 🐙', 'cancelled', '[]', 0, 9223372036854775807, 1e300, -0.01,
     -0.0000000001, '1999-12-31', '1999-12-31 23:59:59', X'', '00000000-0000-0000-0000-000000000000', X'00FF'),
    (3, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

CREATE TABLE no_key (a INTEGER, b TEXT);
INSERT INTO no_key (a, b) VALUES (1, 'one'), (1, 'one'), (2, 'two'), (NULL, NULL);

-- No key either: dbee edits its rows by rowid, even the two that are alike.
CREATE TABLE notes_nokey (title TEXT, body TEXT);
INSERT INTO notes_nokey (title, body) VALUES ('first', 'a note'), ('second', 'another note'), ('second', 'another note');

CREATE VIEW order_totals AS
    SELECT customer_id, count(*) AS orders, sum(total) AS total
    FROM orders
    GROUP BY customer_id;

-- SQLite has no materialized views: a plain view stands in for seed.sql's.
CREATE VIEW customer_counts AS
    SELECT c.id AS customer_id, c.name, count(o.id) AS orders
    FROM customers AS c
    LEFT JOIN orders AS o ON o.customer_id = c.id
    GROUP BY c.id, c.name;

-- SQLite has no partitions: one table holds both years.
CREATE TABLE events (id INTEGER, at DATE);
INSERT INTO events (id, at)
WITH RECURSIVE n(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM n WHERE n < 200)
SELECT n, date('2025-01-01', '+' || (n * 3) || ' days') FROM n;

CREATE TABLE big_rows (id INTEGER PRIMARY KEY, payload TEXT);
INSERT INTO big_rows (id, payload)
WITH RECURSIVE n(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM n WHERE n < 1000000)
SELECT n, 'payload ' || n FROM n;

-- One row with a 20,000,000-character text (hex() of a zeroblob gives the repeats) and a JSON of
-- about the same size.
CREATE TABLE big_cell (id INTEGER PRIMARY KEY, t TEXT, j JSON);
INSERT INTO big_cell (id, t, j)
WITH RECURSIVE n(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM n WHERE n < 320000)
SELECT 1,
       replace(hex(zeroblob(312500)), '00', '0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_'),
       (SELECT json_group_array(json_object('i', n, 's', replace(hex(zeroblob(20)), '0', 'z'))) FROM n);

-- ---------------------------------------------------------------------------------------------
-- many: 300 small tables, t_001 ... t_300 (SQLite has no loops; written out)
-- ---------------------------------------------------------------------------------------------
CREATE TABLE t_001 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_001 (id, v) VALUES (1, 'row of table 1');
CREATE TABLE t_002 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_002 (id, v) VALUES (1, 'row of table 2');
CREATE TABLE t_003 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_003 (id, v) VALUES (1, 'row of table 3');
CREATE TABLE t_004 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_004 (id, v) VALUES (1, 'row of table 4');
CREATE TABLE t_005 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_005 (id, v) VALUES (1, 'row of table 5');
CREATE TABLE t_006 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_006 (id, v) VALUES (1, 'row of table 6');
CREATE TABLE t_007 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_007 (id, v) VALUES (1, 'row of table 7');
CREATE TABLE t_008 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_008 (id, v) VALUES (1, 'row of table 8');
CREATE TABLE t_009 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_009 (id, v) VALUES (1, 'row of table 9');
CREATE TABLE t_010 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_010 (id, v) VALUES (1, 'row of table 10');
CREATE TABLE t_011 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_011 (id, v) VALUES (1, 'row of table 11');
CREATE TABLE t_012 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_012 (id, v) VALUES (1, 'row of table 12');
CREATE TABLE t_013 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_013 (id, v) VALUES (1, 'row of table 13');
CREATE TABLE t_014 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_014 (id, v) VALUES (1, 'row of table 14');
CREATE TABLE t_015 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_015 (id, v) VALUES (1, 'row of table 15');
CREATE TABLE t_016 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_016 (id, v) VALUES (1, 'row of table 16');
CREATE TABLE t_017 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_017 (id, v) VALUES (1, 'row of table 17');
CREATE TABLE t_018 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_018 (id, v) VALUES (1, 'row of table 18');
CREATE TABLE t_019 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_019 (id, v) VALUES (1, 'row of table 19');
CREATE TABLE t_020 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_020 (id, v) VALUES (1, 'row of table 20');
CREATE TABLE t_021 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_021 (id, v) VALUES (1, 'row of table 21');
CREATE TABLE t_022 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_022 (id, v) VALUES (1, 'row of table 22');
CREATE TABLE t_023 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_023 (id, v) VALUES (1, 'row of table 23');
CREATE TABLE t_024 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_024 (id, v) VALUES (1, 'row of table 24');
CREATE TABLE t_025 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_025 (id, v) VALUES (1, 'row of table 25');
CREATE TABLE t_026 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_026 (id, v) VALUES (1, 'row of table 26');
CREATE TABLE t_027 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_027 (id, v) VALUES (1, 'row of table 27');
CREATE TABLE t_028 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_028 (id, v) VALUES (1, 'row of table 28');
CREATE TABLE t_029 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_029 (id, v) VALUES (1, 'row of table 29');
CREATE TABLE t_030 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_030 (id, v) VALUES (1, 'row of table 30');
CREATE TABLE t_031 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_031 (id, v) VALUES (1, 'row of table 31');
CREATE TABLE t_032 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_032 (id, v) VALUES (1, 'row of table 32');
CREATE TABLE t_033 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_033 (id, v) VALUES (1, 'row of table 33');
CREATE TABLE t_034 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_034 (id, v) VALUES (1, 'row of table 34');
CREATE TABLE t_035 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_035 (id, v) VALUES (1, 'row of table 35');
CREATE TABLE t_036 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_036 (id, v) VALUES (1, 'row of table 36');
CREATE TABLE t_037 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_037 (id, v) VALUES (1, 'row of table 37');
CREATE TABLE t_038 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_038 (id, v) VALUES (1, 'row of table 38');
CREATE TABLE t_039 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_039 (id, v) VALUES (1, 'row of table 39');
CREATE TABLE t_040 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_040 (id, v) VALUES (1, 'row of table 40');
CREATE TABLE t_041 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_041 (id, v) VALUES (1, 'row of table 41');
CREATE TABLE t_042 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_042 (id, v) VALUES (1, 'row of table 42');
CREATE TABLE t_043 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_043 (id, v) VALUES (1, 'row of table 43');
CREATE TABLE t_044 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_044 (id, v) VALUES (1, 'row of table 44');
CREATE TABLE t_045 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_045 (id, v) VALUES (1, 'row of table 45');
CREATE TABLE t_046 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_046 (id, v) VALUES (1, 'row of table 46');
CREATE TABLE t_047 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_047 (id, v) VALUES (1, 'row of table 47');
CREATE TABLE t_048 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_048 (id, v) VALUES (1, 'row of table 48');
CREATE TABLE t_049 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_049 (id, v) VALUES (1, 'row of table 49');
CREATE TABLE t_050 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_050 (id, v) VALUES (1, 'row of table 50');
CREATE TABLE t_051 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_051 (id, v) VALUES (1, 'row of table 51');
CREATE TABLE t_052 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_052 (id, v) VALUES (1, 'row of table 52');
CREATE TABLE t_053 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_053 (id, v) VALUES (1, 'row of table 53');
CREATE TABLE t_054 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_054 (id, v) VALUES (1, 'row of table 54');
CREATE TABLE t_055 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_055 (id, v) VALUES (1, 'row of table 55');
CREATE TABLE t_056 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_056 (id, v) VALUES (1, 'row of table 56');
CREATE TABLE t_057 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_057 (id, v) VALUES (1, 'row of table 57');
CREATE TABLE t_058 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_058 (id, v) VALUES (1, 'row of table 58');
CREATE TABLE t_059 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_059 (id, v) VALUES (1, 'row of table 59');
CREATE TABLE t_060 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_060 (id, v) VALUES (1, 'row of table 60');
CREATE TABLE t_061 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_061 (id, v) VALUES (1, 'row of table 61');
CREATE TABLE t_062 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_062 (id, v) VALUES (1, 'row of table 62');
CREATE TABLE t_063 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_063 (id, v) VALUES (1, 'row of table 63');
CREATE TABLE t_064 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_064 (id, v) VALUES (1, 'row of table 64');
CREATE TABLE t_065 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_065 (id, v) VALUES (1, 'row of table 65');
CREATE TABLE t_066 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_066 (id, v) VALUES (1, 'row of table 66');
CREATE TABLE t_067 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_067 (id, v) VALUES (1, 'row of table 67');
CREATE TABLE t_068 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_068 (id, v) VALUES (1, 'row of table 68');
CREATE TABLE t_069 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_069 (id, v) VALUES (1, 'row of table 69');
CREATE TABLE t_070 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_070 (id, v) VALUES (1, 'row of table 70');
CREATE TABLE t_071 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_071 (id, v) VALUES (1, 'row of table 71');
CREATE TABLE t_072 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_072 (id, v) VALUES (1, 'row of table 72');
CREATE TABLE t_073 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_073 (id, v) VALUES (1, 'row of table 73');
CREATE TABLE t_074 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_074 (id, v) VALUES (1, 'row of table 74');
CREATE TABLE t_075 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_075 (id, v) VALUES (1, 'row of table 75');
CREATE TABLE t_076 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_076 (id, v) VALUES (1, 'row of table 76');
CREATE TABLE t_077 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_077 (id, v) VALUES (1, 'row of table 77');
CREATE TABLE t_078 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_078 (id, v) VALUES (1, 'row of table 78');
CREATE TABLE t_079 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_079 (id, v) VALUES (1, 'row of table 79');
CREATE TABLE t_080 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_080 (id, v) VALUES (1, 'row of table 80');
CREATE TABLE t_081 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_081 (id, v) VALUES (1, 'row of table 81');
CREATE TABLE t_082 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_082 (id, v) VALUES (1, 'row of table 82');
CREATE TABLE t_083 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_083 (id, v) VALUES (1, 'row of table 83');
CREATE TABLE t_084 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_084 (id, v) VALUES (1, 'row of table 84');
CREATE TABLE t_085 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_085 (id, v) VALUES (1, 'row of table 85');
CREATE TABLE t_086 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_086 (id, v) VALUES (1, 'row of table 86');
CREATE TABLE t_087 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_087 (id, v) VALUES (1, 'row of table 87');
CREATE TABLE t_088 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_088 (id, v) VALUES (1, 'row of table 88');
CREATE TABLE t_089 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_089 (id, v) VALUES (1, 'row of table 89');
CREATE TABLE t_090 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_090 (id, v) VALUES (1, 'row of table 90');
CREATE TABLE t_091 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_091 (id, v) VALUES (1, 'row of table 91');
CREATE TABLE t_092 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_092 (id, v) VALUES (1, 'row of table 92');
CREATE TABLE t_093 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_093 (id, v) VALUES (1, 'row of table 93');
CREATE TABLE t_094 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_094 (id, v) VALUES (1, 'row of table 94');
CREATE TABLE t_095 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_095 (id, v) VALUES (1, 'row of table 95');
CREATE TABLE t_096 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_096 (id, v) VALUES (1, 'row of table 96');
CREATE TABLE t_097 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_097 (id, v) VALUES (1, 'row of table 97');
CREATE TABLE t_098 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_098 (id, v) VALUES (1, 'row of table 98');
CREATE TABLE t_099 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_099 (id, v) VALUES (1, 'row of table 99');
CREATE TABLE t_100 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_100 (id, v) VALUES (1, 'row of table 100');
CREATE TABLE t_101 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_101 (id, v) VALUES (1, 'row of table 101');
CREATE TABLE t_102 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_102 (id, v) VALUES (1, 'row of table 102');
CREATE TABLE t_103 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_103 (id, v) VALUES (1, 'row of table 103');
CREATE TABLE t_104 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_104 (id, v) VALUES (1, 'row of table 104');
CREATE TABLE t_105 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_105 (id, v) VALUES (1, 'row of table 105');
CREATE TABLE t_106 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_106 (id, v) VALUES (1, 'row of table 106');
CREATE TABLE t_107 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_107 (id, v) VALUES (1, 'row of table 107');
CREATE TABLE t_108 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_108 (id, v) VALUES (1, 'row of table 108');
CREATE TABLE t_109 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_109 (id, v) VALUES (1, 'row of table 109');
CREATE TABLE t_110 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_110 (id, v) VALUES (1, 'row of table 110');
CREATE TABLE t_111 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_111 (id, v) VALUES (1, 'row of table 111');
CREATE TABLE t_112 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_112 (id, v) VALUES (1, 'row of table 112');
CREATE TABLE t_113 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_113 (id, v) VALUES (1, 'row of table 113');
CREATE TABLE t_114 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_114 (id, v) VALUES (1, 'row of table 114');
CREATE TABLE t_115 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_115 (id, v) VALUES (1, 'row of table 115');
CREATE TABLE t_116 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_116 (id, v) VALUES (1, 'row of table 116');
CREATE TABLE t_117 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_117 (id, v) VALUES (1, 'row of table 117');
CREATE TABLE t_118 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_118 (id, v) VALUES (1, 'row of table 118');
CREATE TABLE t_119 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_119 (id, v) VALUES (1, 'row of table 119');
CREATE TABLE t_120 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_120 (id, v) VALUES (1, 'row of table 120');
CREATE TABLE t_121 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_121 (id, v) VALUES (1, 'row of table 121');
CREATE TABLE t_122 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_122 (id, v) VALUES (1, 'row of table 122');
CREATE TABLE t_123 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_123 (id, v) VALUES (1, 'row of table 123');
CREATE TABLE t_124 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_124 (id, v) VALUES (1, 'row of table 124');
CREATE TABLE t_125 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_125 (id, v) VALUES (1, 'row of table 125');
CREATE TABLE t_126 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_126 (id, v) VALUES (1, 'row of table 126');
CREATE TABLE t_127 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_127 (id, v) VALUES (1, 'row of table 127');
CREATE TABLE t_128 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_128 (id, v) VALUES (1, 'row of table 128');
CREATE TABLE t_129 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_129 (id, v) VALUES (1, 'row of table 129');
CREATE TABLE t_130 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_130 (id, v) VALUES (1, 'row of table 130');
CREATE TABLE t_131 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_131 (id, v) VALUES (1, 'row of table 131');
CREATE TABLE t_132 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_132 (id, v) VALUES (1, 'row of table 132');
CREATE TABLE t_133 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_133 (id, v) VALUES (1, 'row of table 133');
CREATE TABLE t_134 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_134 (id, v) VALUES (1, 'row of table 134');
CREATE TABLE t_135 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_135 (id, v) VALUES (1, 'row of table 135');
CREATE TABLE t_136 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_136 (id, v) VALUES (1, 'row of table 136');
CREATE TABLE t_137 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_137 (id, v) VALUES (1, 'row of table 137');
CREATE TABLE t_138 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_138 (id, v) VALUES (1, 'row of table 138');
CREATE TABLE t_139 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_139 (id, v) VALUES (1, 'row of table 139');
CREATE TABLE t_140 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_140 (id, v) VALUES (1, 'row of table 140');
CREATE TABLE t_141 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_141 (id, v) VALUES (1, 'row of table 141');
CREATE TABLE t_142 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_142 (id, v) VALUES (1, 'row of table 142');
CREATE TABLE t_143 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_143 (id, v) VALUES (1, 'row of table 143');
CREATE TABLE t_144 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_144 (id, v) VALUES (1, 'row of table 144');
CREATE TABLE t_145 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_145 (id, v) VALUES (1, 'row of table 145');
CREATE TABLE t_146 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_146 (id, v) VALUES (1, 'row of table 146');
CREATE TABLE t_147 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_147 (id, v) VALUES (1, 'row of table 147');
CREATE TABLE t_148 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_148 (id, v) VALUES (1, 'row of table 148');
CREATE TABLE t_149 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_149 (id, v) VALUES (1, 'row of table 149');
CREATE TABLE t_150 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_150 (id, v) VALUES (1, 'row of table 150');
CREATE TABLE t_151 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_151 (id, v) VALUES (1, 'row of table 151');
CREATE TABLE t_152 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_152 (id, v) VALUES (1, 'row of table 152');
CREATE TABLE t_153 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_153 (id, v) VALUES (1, 'row of table 153');
CREATE TABLE t_154 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_154 (id, v) VALUES (1, 'row of table 154');
CREATE TABLE t_155 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_155 (id, v) VALUES (1, 'row of table 155');
CREATE TABLE t_156 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_156 (id, v) VALUES (1, 'row of table 156');
CREATE TABLE t_157 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_157 (id, v) VALUES (1, 'row of table 157');
CREATE TABLE t_158 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_158 (id, v) VALUES (1, 'row of table 158');
CREATE TABLE t_159 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_159 (id, v) VALUES (1, 'row of table 159');
CREATE TABLE t_160 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_160 (id, v) VALUES (1, 'row of table 160');
CREATE TABLE t_161 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_161 (id, v) VALUES (1, 'row of table 161');
CREATE TABLE t_162 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_162 (id, v) VALUES (1, 'row of table 162');
CREATE TABLE t_163 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_163 (id, v) VALUES (1, 'row of table 163');
CREATE TABLE t_164 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_164 (id, v) VALUES (1, 'row of table 164');
CREATE TABLE t_165 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_165 (id, v) VALUES (1, 'row of table 165');
CREATE TABLE t_166 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_166 (id, v) VALUES (1, 'row of table 166');
CREATE TABLE t_167 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_167 (id, v) VALUES (1, 'row of table 167');
CREATE TABLE t_168 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_168 (id, v) VALUES (1, 'row of table 168');
CREATE TABLE t_169 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_169 (id, v) VALUES (1, 'row of table 169');
CREATE TABLE t_170 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_170 (id, v) VALUES (1, 'row of table 170');
CREATE TABLE t_171 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_171 (id, v) VALUES (1, 'row of table 171');
CREATE TABLE t_172 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_172 (id, v) VALUES (1, 'row of table 172');
CREATE TABLE t_173 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_173 (id, v) VALUES (1, 'row of table 173');
CREATE TABLE t_174 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_174 (id, v) VALUES (1, 'row of table 174');
CREATE TABLE t_175 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_175 (id, v) VALUES (1, 'row of table 175');
CREATE TABLE t_176 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_176 (id, v) VALUES (1, 'row of table 176');
CREATE TABLE t_177 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_177 (id, v) VALUES (1, 'row of table 177');
CREATE TABLE t_178 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_178 (id, v) VALUES (1, 'row of table 178');
CREATE TABLE t_179 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_179 (id, v) VALUES (1, 'row of table 179');
CREATE TABLE t_180 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_180 (id, v) VALUES (1, 'row of table 180');
CREATE TABLE t_181 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_181 (id, v) VALUES (1, 'row of table 181');
CREATE TABLE t_182 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_182 (id, v) VALUES (1, 'row of table 182');
CREATE TABLE t_183 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_183 (id, v) VALUES (1, 'row of table 183');
CREATE TABLE t_184 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_184 (id, v) VALUES (1, 'row of table 184');
CREATE TABLE t_185 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_185 (id, v) VALUES (1, 'row of table 185');
CREATE TABLE t_186 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_186 (id, v) VALUES (1, 'row of table 186');
CREATE TABLE t_187 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_187 (id, v) VALUES (1, 'row of table 187');
CREATE TABLE t_188 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_188 (id, v) VALUES (1, 'row of table 188');
CREATE TABLE t_189 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_189 (id, v) VALUES (1, 'row of table 189');
CREATE TABLE t_190 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_190 (id, v) VALUES (1, 'row of table 190');
CREATE TABLE t_191 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_191 (id, v) VALUES (1, 'row of table 191');
CREATE TABLE t_192 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_192 (id, v) VALUES (1, 'row of table 192');
CREATE TABLE t_193 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_193 (id, v) VALUES (1, 'row of table 193');
CREATE TABLE t_194 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_194 (id, v) VALUES (1, 'row of table 194');
CREATE TABLE t_195 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_195 (id, v) VALUES (1, 'row of table 195');
CREATE TABLE t_196 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_196 (id, v) VALUES (1, 'row of table 196');
CREATE TABLE t_197 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_197 (id, v) VALUES (1, 'row of table 197');
CREATE TABLE t_198 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_198 (id, v) VALUES (1, 'row of table 198');
CREATE TABLE t_199 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_199 (id, v) VALUES (1, 'row of table 199');
CREATE TABLE t_200 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_200 (id, v) VALUES (1, 'row of table 200');
CREATE TABLE t_201 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_201 (id, v) VALUES (1, 'row of table 201');
CREATE TABLE t_202 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_202 (id, v) VALUES (1, 'row of table 202');
CREATE TABLE t_203 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_203 (id, v) VALUES (1, 'row of table 203');
CREATE TABLE t_204 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_204 (id, v) VALUES (1, 'row of table 204');
CREATE TABLE t_205 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_205 (id, v) VALUES (1, 'row of table 205');
CREATE TABLE t_206 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_206 (id, v) VALUES (1, 'row of table 206');
CREATE TABLE t_207 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_207 (id, v) VALUES (1, 'row of table 207');
CREATE TABLE t_208 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_208 (id, v) VALUES (1, 'row of table 208');
CREATE TABLE t_209 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_209 (id, v) VALUES (1, 'row of table 209');
CREATE TABLE t_210 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_210 (id, v) VALUES (1, 'row of table 210');
CREATE TABLE t_211 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_211 (id, v) VALUES (1, 'row of table 211');
CREATE TABLE t_212 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_212 (id, v) VALUES (1, 'row of table 212');
CREATE TABLE t_213 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_213 (id, v) VALUES (1, 'row of table 213');
CREATE TABLE t_214 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_214 (id, v) VALUES (1, 'row of table 214');
CREATE TABLE t_215 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_215 (id, v) VALUES (1, 'row of table 215');
CREATE TABLE t_216 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_216 (id, v) VALUES (1, 'row of table 216');
CREATE TABLE t_217 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_217 (id, v) VALUES (1, 'row of table 217');
CREATE TABLE t_218 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_218 (id, v) VALUES (1, 'row of table 218');
CREATE TABLE t_219 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_219 (id, v) VALUES (1, 'row of table 219');
CREATE TABLE t_220 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_220 (id, v) VALUES (1, 'row of table 220');
CREATE TABLE t_221 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_221 (id, v) VALUES (1, 'row of table 221');
CREATE TABLE t_222 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_222 (id, v) VALUES (1, 'row of table 222');
CREATE TABLE t_223 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_223 (id, v) VALUES (1, 'row of table 223');
CREATE TABLE t_224 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_224 (id, v) VALUES (1, 'row of table 224');
CREATE TABLE t_225 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_225 (id, v) VALUES (1, 'row of table 225');
CREATE TABLE t_226 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_226 (id, v) VALUES (1, 'row of table 226');
CREATE TABLE t_227 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_227 (id, v) VALUES (1, 'row of table 227');
CREATE TABLE t_228 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_228 (id, v) VALUES (1, 'row of table 228');
CREATE TABLE t_229 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_229 (id, v) VALUES (1, 'row of table 229');
CREATE TABLE t_230 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_230 (id, v) VALUES (1, 'row of table 230');
CREATE TABLE t_231 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_231 (id, v) VALUES (1, 'row of table 231');
CREATE TABLE t_232 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_232 (id, v) VALUES (1, 'row of table 232');
CREATE TABLE t_233 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_233 (id, v) VALUES (1, 'row of table 233');
CREATE TABLE t_234 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_234 (id, v) VALUES (1, 'row of table 234');
CREATE TABLE t_235 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_235 (id, v) VALUES (1, 'row of table 235');
CREATE TABLE t_236 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_236 (id, v) VALUES (1, 'row of table 236');
CREATE TABLE t_237 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_237 (id, v) VALUES (1, 'row of table 237');
CREATE TABLE t_238 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_238 (id, v) VALUES (1, 'row of table 238');
CREATE TABLE t_239 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_239 (id, v) VALUES (1, 'row of table 239');
CREATE TABLE t_240 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_240 (id, v) VALUES (1, 'row of table 240');
CREATE TABLE t_241 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_241 (id, v) VALUES (1, 'row of table 241');
CREATE TABLE t_242 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_242 (id, v) VALUES (1, 'row of table 242');
CREATE TABLE t_243 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_243 (id, v) VALUES (1, 'row of table 243');
CREATE TABLE t_244 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_244 (id, v) VALUES (1, 'row of table 244');
CREATE TABLE t_245 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_245 (id, v) VALUES (1, 'row of table 245');
CREATE TABLE t_246 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_246 (id, v) VALUES (1, 'row of table 246');
CREATE TABLE t_247 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_247 (id, v) VALUES (1, 'row of table 247');
CREATE TABLE t_248 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_248 (id, v) VALUES (1, 'row of table 248');
CREATE TABLE t_249 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_249 (id, v) VALUES (1, 'row of table 249');
CREATE TABLE t_250 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_250 (id, v) VALUES (1, 'row of table 250');
CREATE TABLE t_251 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_251 (id, v) VALUES (1, 'row of table 251');
CREATE TABLE t_252 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_252 (id, v) VALUES (1, 'row of table 252');
CREATE TABLE t_253 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_253 (id, v) VALUES (1, 'row of table 253');
CREATE TABLE t_254 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_254 (id, v) VALUES (1, 'row of table 254');
CREATE TABLE t_255 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_255 (id, v) VALUES (1, 'row of table 255');
CREATE TABLE t_256 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_256 (id, v) VALUES (1, 'row of table 256');
CREATE TABLE t_257 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_257 (id, v) VALUES (1, 'row of table 257');
CREATE TABLE t_258 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_258 (id, v) VALUES (1, 'row of table 258');
CREATE TABLE t_259 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_259 (id, v) VALUES (1, 'row of table 259');
CREATE TABLE t_260 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_260 (id, v) VALUES (1, 'row of table 260');
CREATE TABLE t_261 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_261 (id, v) VALUES (1, 'row of table 261');
CREATE TABLE t_262 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_262 (id, v) VALUES (1, 'row of table 262');
CREATE TABLE t_263 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_263 (id, v) VALUES (1, 'row of table 263');
CREATE TABLE t_264 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_264 (id, v) VALUES (1, 'row of table 264');
CREATE TABLE t_265 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_265 (id, v) VALUES (1, 'row of table 265');
CREATE TABLE t_266 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_266 (id, v) VALUES (1, 'row of table 266');
CREATE TABLE t_267 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_267 (id, v) VALUES (1, 'row of table 267');
CREATE TABLE t_268 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_268 (id, v) VALUES (1, 'row of table 268');
CREATE TABLE t_269 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_269 (id, v) VALUES (1, 'row of table 269');
CREATE TABLE t_270 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_270 (id, v) VALUES (1, 'row of table 270');
CREATE TABLE t_271 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_271 (id, v) VALUES (1, 'row of table 271');
CREATE TABLE t_272 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_272 (id, v) VALUES (1, 'row of table 272');
CREATE TABLE t_273 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_273 (id, v) VALUES (1, 'row of table 273');
CREATE TABLE t_274 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_274 (id, v) VALUES (1, 'row of table 274');
CREATE TABLE t_275 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_275 (id, v) VALUES (1, 'row of table 275');
CREATE TABLE t_276 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_276 (id, v) VALUES (1, 'row of table 276');
CREATE TABLE t_277 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_277 (id, v) VALUES (1, 'row of table 277');
CREATE TABLE t_278 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_278 (id, v) VALUES (1, 'row of table 278');
CREATE TABLE t_279 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_279 (id, v) VALUES (1, 'row of table 279');
CREATE TABLE t_280 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_280 (id, v) VALUES (1, 'row of table 280');
CREATE TABLE t_281 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_281 (id, v) VALUES (1, 'row of table 281');
CREATE TABLE t_282 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_282 (id, v) VALUES (1, 'row of table 282');
CREATE TABLE t_283 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_283 (id, v) VALUES (1, 'row of table 283');
CREATE TABLE t_284 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_284 (id, v) VALUES (1, 'row of table 284');
CREATE TABLE t_285 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_285 (id, v) VALUES (1, 'row of table 285');
CREATE TABLE t_286 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_286 (id, v) VALUES (1, 'row of table 286');
CREATE TABLE t_287 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_287 (id, v) VALUES (1, 'row of table 287');
CREATE TABLE t_288 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_288 (id, v) VALUES (1, 'row of table 288');
CREATE TABLE t_289 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_289 (id, v) VALUES (1, 'row of table 289');
CREATE TABLE t_290 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_290 (id, v) VALUES (1, 'row of table 290');
CREATE TABLE t_291 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_291 (id, v) VALUES (1, 'row of table 291');
CREATE TABLE t_292 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_292 (id, v) VALUES (1, 'row of table 292');
CREATE TABLE t_293 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_293 (id, v) VALUES (1, 'row of table 293');
CREATE TABLE t_294 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_294 (id, v) VALUES (1, 'row of table 294');
CREATE TABLE t_295 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_295 (id, v) VALUES (1, 'row of table 295');
CREATE TABLE t_296 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_296 (id, v) VALUES (1, 'row of table 296');
CREATE TABLE t_297 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_297 (id, v) VALUES (1, 'row of table 297');
CREATE TABLE t_298 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_298 (id, v) VALUES (1, 'row of table 298');
CREATE TABLE t_299 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_299 (id, v) VALUES (1, 'row of table 299');
CREATE TABLE t_300 (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t_300 (id, v) VALUES (1, 'row of table 300');

-- ---------------------------------------------------------------------------------------------
-- odd: identifiers that need quoting (quotes, spaces, upper case, non-ASCII, emoji), and a column
-- named rowid
-- ---------------------------------------------------------------------------------------------
CREATE TABLE "Other Ödd" (
    id INTEGER PRIMARY KEY,
    "Label" TEXT
);
INSERT INTO "Other Ödd" (id, "Label") VALUES (1, 'first'), (2, 'second'), (3, 'third');

CREATE TABLE "Odd ""Name"" 🐙" (
    id INTEGER PRIMARY KEY,
    "Total $" NUMERIC,
    "lower UPPER" TEXT,
    "Other Ödd id" INTEGER REFERENCES "Other Ödd" (id)
);
INSERT INTO "Odd ""Name"" 🐙" (id, "Total $", "lower UPPER", "Other Ödd id") VALUES
    (1, 0.5, 'alpha', 1),
    (2, 12.75, 'Beta', 2),
    (3, 100, 'gamma 🐙', 3),
    (4, NULL, NULL, NULL);

-- No key, and `rowid` names a real column: dbee keys the rows by `_rowid_` instead.
CREATE TABLE rowid_taken (rowid TEXT, note TEXT);
INSERT INTO rowid_taken (rowid, note) VALUES ('r1', 'first'), ('r2', 'second');

COMMIT;

ANALYZE;
