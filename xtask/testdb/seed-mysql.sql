-- Seed for the dbee MySQL and MariaDB test servers (`cargo xtask testdb up`): the shapes of
-- seed.sql (Postgres) in MySQL's dialect, the same file on MySQL 5.7, 8.0 and the newest, and on
-- MariaDB 10.11 and the newest. Synthetic data only. The entrypoint runs it once, as root, before
-- the server accepts TCP connections. Tests treat it as read-only; tests that write use database
-- `scratch`.
--
-- Postgres schemas are databases here: shop, many, odd and scratch. What MySQL lacks gets the
-- nearest shape: the enum is an ENUM column, the materialized view a view, the partitioned table
-- a table with partitions. MySQL-only shapes: the MyISAM table `shop.audit_log` and the zero dates
-- of `shop.legacy_dates`.
--
-- No CTEs (MySQL 5.7 has none): the numbers come from `dbee_seed.n`, dropped at the end.
--
-- Sizes: on mysql80 and mariadbnew (the day's servers) `big_rows` holds 1,000,000 rows and
-- `big_cell` its 20 MB row; elsewhere 30,000 rows and no row in `big_cell`, to keep `testdb up`
-- light on the CPU. compose.yml sets each service's hostname to its name; the two servers' rows in
-- SERVICES (xtask/src/testdb.rs) say `full_seed`, and `testdb up` checks that they agree.

SET NAMES utf8mb4;
-- MariaDB builds JSON_ARRAYAGG on GROUP_CONCAT, which this caps.
SET SESSION group_concat_max_len = 67108864;
SET @full = @@hostname IN ('mysql80', 'mariadbnew');
SET @big_rows = IF(@full, 1000000, 30000);

-- ---------------------------------------------------------------------------------------------
-- dbee_seed: the numbers 1 .. @big_rows, and a procedure for `many`; dropped at the end
-- ---------------------------------------------------------------------------------------------
CREATE DATABASE dbee_seed CHARACTER SET utf8mb4;
CREATE TABLE dbee_seed.digit (d INT PRIMARY KEY);
INSERT INTO dbee_seed.digit (d) VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9);
CREATE TABLE dbee_seed.n (n INT PRIMARY KEY);
INSERT INTO dbee_seed.n (n)
SELECT 1 + a.d + 10 * b.d + 100 * c.d + 1000 * d.d + 10000 * e.d + 100000 * f.d AS n
FROM dbee_seed.digit AS a, dbee_seed.digit AS b, dbee_seed.digit AS c,
     dbee_seed.digit AS d, dbee_seed.digit AS e, dbee_seed.digit AS f
WHERE 1 + a.d + 10 * b.d + 100 * c.d + 1000 * d.d + 10000 * e.d + 100000 * f.d <= @big_rows;

-- ---------------------------------------------------------------------------------------------
-- shop: a small order system with the types, keys and sizes the client has to cope with
-- ---------------------------------------------------------------------------------------------
CREATE DATABASE shop CHARACTER SET utf8mb4;

CREATE TABLE shop.customers (
    id INT AUTO_INCREMENT PRIMARY KEY,
    name TEXT NOT NULL,
    email VARCHAR(200) UNIQUE,
    created_at TIMESTAMP NULL DEFAULT CURRENT_TIMESTAMP,
    note TEXT
);

INSERT INTO shop.customers (id, name, email, created_at, note)
SELECT n,
       CONCAT('Customer ', n),
       CONCAT('customer', n, '@example.com'),
       TIMESTAMP '2025-01-01 09:00:00' + INTERVAL (n * 7) HOUR,
       CASE WHEN n % 5 = 0 THEN NULL ELSE CONCAT('note for customer ', n) END
FROM dbee_seed.n
WHERE n <= 500
ORDER BY n;

CREATE TABLE shop.orders (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    customer_id INT,
    dealer_id INT,
    status ENUM('new', 'paid', 'completed', 'cancelled'),
    total DECIMAL(12,2),
    billing JSON,
    meta JSON,
    shipped_at DATE,
    raw_json_text TEXT,
    FOREIGN KEY (customer_id) REFERENCES shop.customers (id)
);

-- Dealer 42 has completed orders above 100 (the filter example): dealer and status cycle with
-- different periods, so every combination occurs. `meta` holds a JSON string, as in seed.sql.
INSERT INTO shop.orders (id, customer_id, dealer_id, status, total, billing, meta, shipped_at, raw_json_text)
SELECT n,
       1 + (n - 1) % 500,
       CASE WHEN n % 97 = 0 THEN NULL ELSE 1 + n % 50 END,
       status,
       ROUND(((n * 7919) % 50000) / 100.0 + 1, 2),
       JSON_OBJECT(
           'name', CONCAT('Customer ', 1 + (n - 1) % 500),
           'address', JSON_OBJECT(
               'street', CONCAT(n, ' Example Street'),
               'city', CONCAT('City ', n % 20),
               'zip', LPAD(n % 100000, 5, '0'),
               'geo', JSON_OBJECT('lat', ROUND(59 + (n % 100) / 100.0, 2), 'lon', ROUND(24 + (n % 100) / 100.0, 2))),
           'items', JSON_ARRAY(
               JSON_OBJECT('sku', CONCAT('SKU-', n), 'qty', 1 + n % 3),
               JSON_OBJECT('sku', CONCAT('SKU-', n + 1), 'qty', 1)),
           'paid', JSON_EXTRACT(IF(status IN ('paid', 'completed'), 'true', 'false'), '$')),
       JSON_QUOTE(CONCAT('{"a":', 1 + (n - 1) % 3, '}')),
       CASE WHEN status IN ('completed', 'cancelled') THEN DATE '2025-01-01' + INTERVAL (n % 600) DAY END,
       '{"k":[1,2]}'
FROM (
    SELECT n, ELT(1 + (n DIV 3) % 4, 'new', 'paid', 'completed', 'cancelled') AS status
    FROM dbee_seed.n
    WHERE n <= 5000
) AS s
ORDER BY n;

CREATE TABLE shop.order_items (
    order_id BIGINT,
    line_no INT,
    product TEXT,
    qty INT,
    PRIMARY KEY (order_id, line_no),
    FOREIGN KEY (order_id) REFERENCES shop.orders (id)
);

INSERT INTO shop.order_items (order_id, line_no, product, qty)
SELECT o.id, l.n, CONCAT('Product ', (o.id * 31 + l.n) % 200), 1 + (o.id + l.n) % 5
FROM shop.orders AS o
JOIN dbee_seed.n AS l ON l.n <= 1 + o.id % 3
ORDER BY o.id, l.n;

CREATE TABLE shop.shipments (
    id INT AUTO_INCREMENT PRIMARY KEY,
    order_id BIGINT,
    line_no INT,
    FOREIGN KEY (order_id, line_no) REFERENCES shop.order_items (order_id, line_no)
);

-- The ids seed.sql's serial gives: the n-th shipment is for order 2n.
INSERT INTO shop.shipments (id, order_id, line_no)
SELECT order_id DIV 2, order_id, line_no FROM shop.order_items WHERE line_no = 1 AND order_id % 2 = 0 ORDER BY order_id;

-- One column per kind of type: numbers, BOOL (tinyint(1)), strings, TEXT, JSON (on MariaDB a
-- LONGTEXT with a json_valid check), dates and times, binary, a geometry, and a generated column.
CREATE TABLE shop.all_types (
    id INT AUTO_INCREMENT PRIMARY KEY,
    note TEXT,
    st ENUM('new', 'paid', 'completed', 'cancelled'),
    doc JSON,
    flag BOOL,
    tags SET('x', 'y z', 'quote"d'),
    iv TIME,
    y YEAR,
    m DECIMAL(15,2),
    n DECIMAL(30,10),
    f DOUBLE,
    big BIGINT UNSIGNED,
    d DATE,
    ts DATETIME(3),
    b BLOB,
    bits BIT(8),
    u CHAR(36),
    ip VARCHAR(45),
    pt POINT,
    total DECIMAL(31,10) GENERATED ALWAYS AS (n * 2) VIRTUAL
);

INSERT INTO shop.all_types (id, note, st, doc, flag, tags, iv, y, m, n, f, big, d, ts, b, bits, u, ip, pt) VALUES
    (1, 'first row', 'paid', '{"a": {"b": [1, 2, 3]}, "s": "text"}', TRUE, 'x,y z', '26:03:04', 2025, 12.34,
     12345678901234567890.0123456789, 3.141592653589793, 42, '2025-06-01', '2025-06-01 12:34:56.789', X'DEADBEEF',
     b'10100101', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '192.168.0.1/24', ST_GeomFromText('POINT(1.5 -2)')),
    (2, 'second row: edge values', 'cancelled', '[]', FALSE, '', '-838:59:59', 1901, -0.01,
     -0.0000000001, -1.7976931348623157e308, 18446744073709551615, '1000-01-01', '1999-12-31 23:59:59.000', X'',
     b'0', '00000000-0000-0000-0000-000000000000', '::1', ST_GeomFromText('POINT(0 0)')),
    (3, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

CREATE TABLE shop.no_key (a INT, b TEXT);
INSERT INTO shop.no_key (a, b) VALUES (1, 'one'), (1, 'one'), (2, 'two'), (NULL, NULL);

CREATE VIEW shop.order_totals AS
    SELECT customer_id, COUNT(*) AS orders, SUM(total) AS total
    FROM shop.orders
    GROUP BY customer_id;

-- MySQL has no materialized views: a plain view stands in for seed.sql's.
CREATE VIEW shop.customer_counts AS
    SELECT c.id AS customer_id, c.name, COUNT(o.id) AS orders
    FROM shop.customers AS c
    LEFT JOIN shop.orders AS o ON o.customer_id = c.id
    GROUP BY c.id, c.name;

-- MySQL's partitions are parts of one table, not tables of their own as in Postgres.
CREATE TABLE shop.events (id INT, at DATE)
    PARTITION BY RANGE COLUMNS (at) (
        PARTITION p2025 VALUES LESS THAN ('2026-01-01'),
        PARTITION p2026 VALUES LESS THAN ('2027-01-01'));
INSERT INTO shop.events (id, at)
SELECT n, DATE '2025-01-01' + INTERVAL (n * 3) DAY FROM dbee_seed.n WHERE n <= 200 ORDER BY n;

CREATE TABLE shop.big_rows (id INT PRIMARY KEY, payload TEXT);
INSERT INTO shop.big_rows (id, payload)
SELECT n, CONCAT('payload ', n) FROM dbee_seed.n ORDER BY n;

-- On the day's servers one row with a 20,000,000-character LONGTEXT and a JSON of about the same
-- size as text (every MySQL service runs with max_allowed_packet 64 MiB); elsewhere no row.
CREATE TABLE shop.big_cell (id INT PRIMARY KEY, t LONGTEXT, j JSON);
INSERT INTO shop.big_cell (id, t, j)
SELECT 1,
       REPEAT('0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-_', 312500),
       (SELECT JSON_ARRAYAGG(JSON_OBJECT('i', n, 's', REPEAT('z', 40))) FROM dbee_seed.n WHERE n <= 320000)
FROM DUAL
WHERE @full;

-- MyISAM keeps no transactions, so dbee's grid shows it read-only; it has a key all the same.
CREATE TABLE shop.audit_log (
    id INT AUTO_INCREMENT PRIMARY KEY,
    at DATETIME NOT NULL,
    action VARCHAR(50) NOT NULL,
    detail TEXT
) ENGINE = MyISAM;
INSERT INTO shop.audit_log (id, at, action, detail) VALUES
    (1, '2025-01-01 09:00:00', 'login', 'customer 1'),
    (2, '2025-01-01 09:05:00', 'order', 'order 1'),
    (3, '2025-01-02 10:00:00', 'logout', NULL);

-- Zero dates, as servers with a loose sql_mode let them in; today's strict modes refuse them.
SET @seed_sql_mode = @@SESSION.sql_mode;
SET SESSION sql_mode = '';
CREATE TABLE shop.legacy_dates (
    id INT PRIMARY KEY,
    name VARCHAR(50),
    d DATE NOT NULL DEFAULT '0000-00-00',
    dt DATETIME NULL DEFAULT '0000-00-00 00:00:00'
);
INSERT INTO shop.legacy_dates (id, name, d, dt) VALUES
    (1, 'zero date', '0000-00-00', '0000-00-00 00:00:00'),
    (2, 'zero day', '2025-01-00', '2025-01-00 00:00:00'),
    (3, 'ordinary', '2025-01-02', '2025-01-02 03:04:05');
SET SESSION sql_mode = @seed_sql_mode;

-- ---------------------------------------------------------------------------------------------
-- many: 300 small tables, t_001 ... t_300
-- ---------------------------------------------------------------------------------------------
CREATE DATABASE many CHARACTER SET utf8mb4;

DELIMITER //
CREATE PROCEDURE dbee_seed.make_many()
BEGIN
    DECLARE i INT DEFAULT 1;
    WHILE i <= 300 DO
        SET @t = CONCAT('many.t_', LPAD(i, 3, '0'));
        SET @s = CONCAT('CREATE TABLE ', @t, ' (id INT PRIMARY KEY, v TEXT)');
        PREPARE stmt FROM @s;
        EXECUTE stmt;
        DEALLOCATE PREPARE stmt;
        SET @s = CONCAT('INSERT INTO ', @t, ' (id, v) VALUES (1, ''row of table ', i, ''')');
        PREPARE stmt FROM @s;
        EXECUTE stmt;
        DEALLOCATE PREPARE stmt;
        SET i = i + 1;
    END WHILE;
END//
DELIMITER ;
CALL dbee_seed.make_many();

-- ---------------------------------------------------------------------------------------------
-- odd: identifiers that need quoting (quotes, spaces, upper case, non-ASCII, a backtick). MySQL
-- identifiers hold only the Basic Multilingual Plane, so no emoji in names; values may hold one.
-- ---------------------------------------------------------------------------------------------
CREATE DATABASE odd CHARACTER SET utf8mb4;

CREATE TABLE odd.`Other Ödd` (
    id INT PRIMARY KEY,
    `Label` TEXT
);
INSERT INTO odd.`Other Ödd` (id, `Label`) VALUES (1, 'first'), (2, 'second'), (3, 'third');

CREATE TABLE odd.`Odd "Name" é` (
    id INT PRIMARY KEY,
    `Total $` DECIMAL(10,2),
    `lower UPPER` TEXT,
    `Other Ödd id` INT,
    `back``tick` TEXT,
    FOREIGN KEY (`Other Ödd id`) REFERENCES odd.`Other Ödd` (id)
);
INSERT INTO odd.`Odd "Name" é` (id, `Total $`, `lower UPPER`, `Other Ödd id`, `back``tick`) VALUES
    (1, 0.5, 'alpha', 1, 'x`y'),
    (2, 12.75, 'Beta', 2, 'plain'),
    (3, 100, 'gamma 🐝', 3, NULL),
    (4, NULL, NULL, NULL, NULL);

-- ---------------------------------------------------------------------------------------------
-- scratch: empty, for tests that write (each test creates and drops its own table)
-- ---------------------------------------------------------------------------------------------
CREATE DATABASE scratch CHARACTER SET utf8mb4;

DROP DATABASE dbee_seed;

ANALYZE TABLE shop.customers, shop.orders, shop.order_items, shop.shipments, shop.all_types, shop.no_key,
    shop.events, shop.big_rows, shop.big_cell, shop.audit_log, shop.legacy_dates;
