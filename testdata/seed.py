"""
Generate synthetic test data for veritable.

Creates tables in both PostgreSQL (Docker) and DuckDB with:
- Identical tables (should diff as equal)
- Tables with inserted rows (only in dst)
- Tables with deleted rows (only in src)
- Tables with modified rows (same key, different values)
- Mixed diffs (inserts + deletes + updates)
- Large table (100k rows) for perf testing

Usage:
    pip install -r requirements.txt
    python seed.py
"""

import os
import random
from datetime import datetime, timedelta
from decimal import Decimal

import duckdb
import psycopg2
from faker import Faker

fake = Faker()
Faker.seed(42)
random.seed(42)

# --- Config ---
PG_HOST = os.getenv("POSTGRES_HOST", "localhost")
PG_PORT = os.getenv("POSTGRES_PORT", "5432")
PG_USER = os.getenv("POSTGRES_USER", "postgres")
PG_PASS = os.getenv("POSTGRES_PASSWORD", "340fd5c70c687b4e622aac22df")
PG_DB = os.getenv("POSTGRES_DB", "veritable")

DUCK_PATH = os.path.join(os.path.dirname(__file__), "..", "data", "duckdb", "veritable.duckdb")

NUM_ROWS = 10_000
NUM_ROWS_LARGE = 100_000


def get_pg_conn():
    return psycopg2.connect(
        host=PG_HOST, port=PG_PORT,
        user=PG_USER, password=PG_PASS,
        dbname=PG_DB
    )


def get_duck_conn():
    os.makedirs(os.path.dirname(DUCK_PATH), exist_ok=True)
    return duckdb.connect(DUCK_PATH)


def generate_customers(n):
    """Generate customer rows: id, name, email, created_at, balance, active"""
    rows = []
    for i in range(1, n + 1):
        rows.append((
            i,
            fake.name(),
            fake.email(),
            fake.date_time_between(start_date="-2y", end_date="now"),
            round(random.uniform(0, 50000), 2),
            random.choice([True, False]),
        ))
    return rows


def generate_orders(n, max_customer_id):
    """Generate order rows: id, customer_id, amount, status, ordered_at"""
    statuses = ["pending", "shipped", "delivered", "cancelled"]
    rows = []
    for i in range(1, n + 1):
        rows.append((
            i,
            random.randint(1, max_customer_id),
            round(random.uniform(5, 2000), 2),
            random.choice(statuses),
            fake.date_time_between(start_date="-1y", end_date="now"),
        ))
    return rows


def generate_products(n):
    """Generate product rows: id, name, sku, price, weight_kg, description"""
    rows = []
    for i in range(1, n + 1):
        rows.append((
            i,
            fake.catch_phrase(),
            fake.bothify("???-#####").upper(),
            round(random.uniform(1, 500), 2),
            round(random.uniform(0.1, 50.0), 3),
            fake.text(max_nb_chars=200) if random.random() > 0.3 else None,
        ))
    return rows


# --- Table definitions ---

CUSTOMERS_DDL = """
CREATE TABLE IF NOT EXISTS customers (
    id INTEGER PRIMARY KEY,
    name VARCHAR(200) NOT NULL,
    email VARCHAR(200) NOT NULL,
    created_at TIMESTAMP NOT NULL,
    balance DECIMAL(12,2) NOT NULL,
    active BOOLEAN NOT NULL
)
"""

ORDERS_DDL = """
CREATE TABLE IF NOT EXISTS orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL,
    amount DECIMAL(12,2) NOT NULL,
    status VARCHAR(20) NOT NULL,
    ordered_at TIMESTAMP NOT NULL
)
"""

PRODUCTS_DDL = """
CREATE TABLE IF NOT EXISTS products (
    id INTEGER PRIMARY KEY,
    name VARCHAR(300) NOT NULL,
    sku VARCHAR(20) NOT NULL,
    price DECIMAL(10,2) NOT NULL,
    weight_kg DECIMAL(8,3) NOT NULL,
    description TEXT
)
"""


def drop_all(cursor, tables):
    for t in tables:
        cursor.execute(f"DROP TABLE IF EXISTS {t}")


def insert_customers(cursor, rows, table="customers"):
    for r in rows:
        cursor.execute(
            f"INSERT INTO {table} (id, name, email, created_at, balance, active) VALUES (%s, %s, %s, %s, %s, %s)",
            r
        )


def insert_orders(cursor, rows, table="orders"):
    for r in rows:
        cursor.execute(
            f"INSERT INTO {table} (id, customer_id, amount, status, ordered_at) VALUES (%s, %s, %s, %s, %s)",
            r
        )


def insert_products(cursor, rows, table="products"):
    for r in rows:
        cursor.execute(
            f"INSERT INTO {table} (id, name, sku, price, weight_kg, description) VALUES (%s, %s, %s, %s, %s, %s)",
            r
        )


def insert_customers_duck(conn, rows, table="customers"):
    conn.executemany(
        f"INSERT INTO {table} VALUES (?, ?, ?, ?, ?, ?)", rows
    )


def insert_orders_duck(conn, rows, table="orders"):
    conn.executemany(
        f"INSERT INTO {table} VALUES (?, ?, ?, ?, ?)", rows
    )


def insert_products_duck(conn, rows, table="products"):
    conn.executemany(
        f"INSERT INTO {table} VALUES (?, ?, ?, ?, ?, ?)", rows
    )


def mutate_rows(rows, num_updates, num_deletes, num_inserts, next_id):
    """
    Apply mutations to a copy of rows.
    Returns (mutated_rows, changes_summary).
    """
    mutated = list(rows)
    changes = {"updated": [], "deleted": [], "inserted": []}

    # Updates: modify balance (col index 4) for random rows
    update_indices = random.sample(range(len(mutated)), min(num_updates, len(mutated)))
    for idx in update_indices:
        row = list(mutated[idx])
        row[4] = round(row[4] + random.uniform(-1000, 1000), 2)
        changes["updated"].append(row[0])
        mutated[idx] = tuple(row)

    # Deletes: remove random rows
    delete_indices = sorted(
        random.sample(range(len(mutated)), min(num_deletes, len(mutated))),
        reverse=True
    )
    for idx in delete_indices:
        changes["deleted"].append(mutated[idx][0])
        mutated.pop(idx)

    # Inserts: add new rows at the end
    for i in range(num_inserts):
        new_row = (
            next_id + i,
            fake.name(),
            fake.email(),
            fake.date_time_between(start_date="-2y", end_date="now"),
            round(random.uniform(0, 50000), 2),
            random.choice([True, False]),
        )
        changes["inserted"].append(new_row[0])
        mutated.append(new_row)

    return mutated, changes


def seed_postgres(customers, orders, products, customers_modified, customers_extra_inserts):
    """Seed PostgreSQL with all test tables."""
    print("Connecting to PostgreSQL...")
    conn = get_pg_conn()
    conn.autocommit = True
    cur = conn.cursor()

    # Clean slate
    drop_all(cur, [
        "customers_src", "customers_dst",
        "customers_identical_src", "customers_identical_dst",
        "orders_src", "orders_dst",
        "products_src", "products_dst",
        "customers_large_src", "customers_large_dst",
    ])

    # --- Identical tables (should produce 0 diffs) ---
    cur.execute(CUSTOMERS_DDL.replace("customers", "customers_identical_src"))
    cur.execute(CUSTOMERS_DDL.replace("customers", "customers_identical_dst"))
    print("  Inserting identical customers (src & dst)...")
    insert_customers(cur, customers, "customers_identical_src")
    insert_customers(cur, customers, "customers_identical_dst")

    # --- Modified table (updates + deletes + inserts) ---
    cur.execute(CUSTOMERS_DDL.replace("customers", "customers_src"))
    cur.execute(CUSTOMERS_DDL.replace("customers", "customers_dst"))
    print("  Inserting customers_src (original)...")
    insert_customers(cur, customers, "customers_src")
    print("  Inserting customers_dst (modified)...")
    insert_customers(cur, customers_modified, "customers_dst")

    # --- Orders (dst has extra inserts only) ---
    cur.execute(ORDERS_DDL.replace("orders", "orders_src"))
    cur.execute(ORDERS_DDL.replace("orders", "orders_dst"))
    print("  Inserting orders...")
    insert_orders(cur, orders, "orders_src")
    insert_orders(cur, orders + customers_extra_inserts, "orders_dst")

    # --- Products (identical, for cross-engine test) ---
    cur.execute(PRODUCTS_DDL.replace("products", "products_src"))
    print("  Inserting products...")
    insert_products(cur, products, "products_src")

    cur.close()
    conn.close()
    print("  PostgreSQL done.")


def seed_duckdb(customers, orders, products, customers_modified, customers_extra_inserts):
    """Seed DuckDB with matching test tables."""
    print("Connecting to DuckDB...")
    conn = get_duck_conn()

    # Clean slate
    for t in [
        "customers_src", "customers_dst",
        "customers_identical_src", "customers_identical_dst",
        "orders_src", "orders_dst",
        "products_dst",
        "customers_large_src", "customers_large_dst",
    ]:
        conn.execute(f"DROP TABLE IF EXISTS {t}")

    # --- Identical tables ---
    conn.execute(CUSTOMERS_DDL.replace("customers", "customers_identical_src"))
    conn.execute(CUSTOMERS_DDL.replace("customers", "customers_identical_dst"))
    print("  Inserting identical customers (src & dst)...")
    insert_customers_duck(conn, customers, "customers_identical_src")
    insert_customers_duck(conn, customers, "customers_identical_dst")

    # --- Modified table ---
    conn.execute(CUSTOMERS_DDL.replace("customers", "customers_src"))
    conn.execute(CUSTOMERS_DDL.replace("customers", "customers_dst"))
    print("  Inserting customers_src (original)...")
    insert_customers_duck(conn, customers, "customers_src")
    print("  Inserting customers_dst (modified)...")
    insert_customers_duck(conn, customers_modified, "customers_dst")

    # --- Orders (dst has extra inserts) ---
    conn.execute(ORDERS_DDL.replace("orders", "orders_src"))
    conn.execute(ORDERS_DDL.replace("orders", "orders_dst"))
    print("  Inserting orders...")
    insert_orders_duck(conn, orders, "orders_src")
    insert_orders_duck(conn, orders + customers_extra_inserts, "orders_dst")

    # --- Products (for cross-engine: PG src, DuckDB dst) ---
    conn.execute(PRODUCTS_DDL.replace("products", "products_dst"))
    print("  Inserting products...")
    insert_products_duck(conn, products, "products_dst")

    conn.close()
    print("  DuckDB done.")


def seed_large_tables(n=NUM_ROWS_LARGE):
    """Generate a large table pair with known mutations for perf testing."""
    print(f"Generating large table ({n} rows)...")
    large_customers = generate_customers(n)

    # Apply known mutations: 500 updates, 200 deletes, 300 inserts
    large_modified, large_changes = mutate_rows(
        large_customers,
        num_updates=500,
        num_deletes=200,
        num_inserts=300,
        next_id=n + 1
    )

    print(f"  Mutations: {len(large_changes['updated'])} updates, "
          f"{len(large_changes['deleted'])} deletes, "
          f"{len(large_changes['inserted'])} inserts")

    # PostgreSQL
    print("  Seeding large tables in PostgreSQL...")
    conn = get_pg_conn()
    conn.autocommit = True
    cur = conn.cursor()
    drop_all(cur, ["customers_large_src", "customers_large_dst"])
    cur.execute(CUSTOMERS_DDL.replace("customers", "customers_large_src"))
    cur.execute(CUSTOMERS_DDL.replace("customers", "customers_large_dst"))
    insert_customers(cur, large_customers, "customers_large_src")
    insert_customers(cur, large_modified, "customers_large_dst")
    cur.close()
    conn.close()

    # DuckDB
    print("  Seeding large tables in DuckDB...")
    duck = get_duck_conn()
    duck.execute("DROP TABLE IF EXISTS customers_large_src")
    duck.execute("DROP TABLE IF EXISTS customers_large_dst")
    duck.execute(CUSTOMERS_DDL.replace("customers", "customers_large_src"))
    duck.execute(CUSTOMERS_DDL.replace("customers", "customers_large_dst"))
    insert_customers_duck(duck, large_customers, "customers_large_src")
    insert_customers_duck(duck, large_modified, "customers_large_dst")
    duck.close()

    # Save mutation manifest
    manifest_path = os.path.join(os.path.dirname(__file__), "large_mutations.txt")
    with open(manifest_path, "w") as f:
        f.write(f"total_src_rows: {n}\n")
        f.write(f"total_dst_rows: {len(large_modified)}\n")
        f.write(f"updated_ids: {large_changes['updated'][:20]}...\n")
        f.write(f"deleted_ids: {large_changes['deleted'][:20]}...\n")
        f.write(f"inserted_ids: {large_changes['inserted'][:20]}...\n")
        f.write(f"expected_diffs: {len(large_changes['updated']) + len(large_changes['deleted']) + len(large_changes['inserted'])}\n")
    print(f"  Manifest written to {manifest_path}")


def main():
    print("=== Veritable Test Data Seeder ===\n")

    # Generate base data
    print(f"Generating {NUM_ROWS} rows per table...")
    customers = generate_customers(NUM_ROWS)
    orders = generate_orders(NUM_ROWS, max_customer_id=NUM_ROWS)
    products = generate_products(NUM_ROWS)

    # Mutate customers: 200 updates, 100 deletes, 150 inserts
    customers_modified, changes = mutate_rows(
        customers,
        num_updates=200,
        num_deletes=100,
        num_inserts=150,
        next_id=NUM_ROWS + 1
    )
    print(f"  Customer mutations: {len(changes['updated'])} updates, "
          f"{len(changes['deleted'])} deletes, "
          f"{len(changes['inserted'])} inserts")
    print(f"  Expected diffs: {len(changes['updated']) + len(changes['deleted']) + len(changes['inserted'])}")

    # Generate extra order inserts for dst
    extra_orders = generate_orders(50, max_customer_id=NUM_ROWS)
    # Rebase IDs
    extra_orders = [(NUM_ROWS + i + 1, *r[1:]) for i, r in enumerate(extra_orders)]

    # Seed both databases
    seed_postgres(customers, orders, products, customers_modified, extra_orders)
    seed_duckdb(customers, orders, products, customers_modified, extra_orders)

    # Large perf table
    seed_large_tables()

    print("\n=== Done ===")
    print("\nTest scenarios:")
    print("  1. customers_identical_src vs customers_identical_dst  → 0 diffs (fast-exit)")
    print("  2. customers_src vs customers_dst                      → 450 diffs (updates+deletes+inserts)")
    print("  3. orders_src vs orders_dst                            → 50 diffs (inserts only)")
    print("  4. products_src (PG) vs products_dst (DuckDB)          → 0 diffs (cross-engine)")
    print("  5. customers_large_src vs customers_large_dst          → 1000 diffs (perf test)")


if __name__ == "__main__":
    main()
